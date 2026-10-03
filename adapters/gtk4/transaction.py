"""Bounded source-thread barrier for explicitly registered cooperating surfaces.

The caller owns native topology/semantic reads and actual snapshot requests.
This module never samples a paintable, collects unrelated latest frames, or
authenticates a peer. Only fresh snapshot callbacks for an active barrier can
complete it. The GTK fixture supplies the serialized source actor.
"""
from collections import deque
import json
import time


MAX_SURFACES = 4
MAX_RECORDS = 20_000
MAX_RESOURCES = 4096
METADATA_CHARGE = 16 << 20
MAX_UNION_BYTES = 32 << 20


class BarrierRejected(ValueError):
    pass


class Reservation:
    def __init__(self, budget, charge):
        self.budget, self.charge = budget, charge
        self.released = False

    def close(self):
        if not self.released:
            self.budget.used -= self.charge
            self.released = True


class CaptureBudget:
    """Conservative producer capture quota, separate from GTK app memory.

    Source frame slots, candidates, boundary reads, encoding and the two file
    resource closures share one counter. Files are charged before ownership,
    even when their body is read only during the later artifact handoff.
    """
    def __init__(self, limit=256 << 20):
        self.limit, self.used, self.peak = limit, 0, 0

    def reserve(self, charge):
        if charge < 0 or self.used + charge > self.limit:
            raise BarrierRejected('capture-memory-ceiling')
        self.used += charge
        self.peak = max(self.peak, self.used)
        return Reservation(self, charge)


class ArtifactQuota:
    """Finite fixture output ownership, including local diagnostic artifacts."""
    def __init__(self, limit=64 << 20, max_files=256):
        self.limit, self.max_files, self.used = limit, max_files, 0
        self.files = {}

    def write(self, path, body):
        previous = self.files.get(path, 0)
        charge = self.used - previous + len(body)
        if charge > self.limit or (path not in self.files and len(self.files) >= self.max_files):
            raise BarrierRejected('fixture-artifact-ceiling')
        # Account before retaining the new file. On failure the fixture aborts
        # without claiming a coherent publication; previous output stays owned.
        self.used = charge
        self.files[path] = len(body)
        path.write_bytes(body)


def stamp(value):
    # Source callback data, never peer JSON. Reserve METADATA_CHARGE before
    # invoking the native oracle or serializing its bounded read result.
    encoded = json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()
    if len(encoded) > 1 << 20:
        raise BarrierRejected('boundary-metadata-ceiling')
    return encoded


class CaptureTicket:
    def __init__(self, owner, key, nonce, reservation):
        self.owner, self.key, self.nonce = owner, key, nonce
        self.reservation = reservation
        self.finished = False

    def close(self):
        if not self.finished:
            self.reservation.close()
            self.finished = True


class CaptureTransaction:
    def __init__(self, source, oracle, request_snapshots, committed, *, budget=None,
                 timeout=2.0, max_restarts=4, clock=time.monotonic, on_candidate=None):
        if not 0 < timeout <= 5 or not 0 <= max_restarts <= 8:
            raise ValueError('invalid capture barrier limits')
        self.source = source
        self.oracle, self.request_snapshots, self.committed = oracle, request_snapshots, committed
        self.on_candidate = on_candidate
        self.budget = budget or CaptureBudget()
        self.timeout, self.max_restarts, self.clock = timeout, max_restarts, clock
        self.metadata_reservation = self.budget.reserve(1 << 20)
        self.registered = {}
        self.events = deque(maxlen=64)
        self.nonce = 0
        self.active = False
        self.closed = False
        self.committing = False
        self.candidates = {}
        self.tickets = {}
        self.pending_union = {}
        self.body_reservations = {}
        self.union_bytes = 0
        self.boundary_reservation = None
        self.encoding_reservation = None
        self.deadline = 0
        self.restarts = 0
        self.boundary = None
        self.required = set()

    def register(self, key, capture):
        if self.active or self.closed or key in self.registered or len(self.registered) >= MAX_SURFACES:
            raise BarrierRejected('capture-registration')
        # The capture's retained last_frame has a permanent conservative slot.
        slot = self.budget.reserve(METADATA_CHARGE)
        self.registered[key] = (capture, slot)

    def begin(self):
        if self.closed or self.active:
            raise BarrierRejected('capture-request-overlap-or-closed')
        self.deadline = self.clock() + self.timeout
        self.restarts = 0
        self._start()

    def _read_boundary(self):
        facts = self.oracle()
        if facts.get('incarnation') != self.source.identity or facts.get('source_revision') != self.source.source_revision:
            raise BarrierRejected('source-boundary-incarnation-or-clock')
        surfaces = facts.get('surfaces', {})
        if set(surfaces) != set(self.registered):
            raise BarrierRejected('source-boundary-registration')
        return facts, stamp(facts)

    def _start(self):
        if self.nonce >= (1 << 64) - 1:
            raise BarrierRejected("capture-nonce-ceiling")
        self.nonce += 1
        nonce = self.nonce
        self.active = True
        try:
            self.boundary_reservation = self.budget.reserve(METADATA_CHARGE)
            facts, self.boundary = self._read_boundary()
            self.required = {key for key, surface in facts['surfaces'].items() if surface['alive'] and surface['mapped']}
            # Reserve every candidate before any requested native capture.
            for key in sorted(self.required):
                self.tickets[key] = CaptureTicket(self, key, self.nonce, self.budget.reserve(METADATA_CHARGE))
            self.events.append({'event': 'begin', 'nonce': self.nonce, 'required': sorted(self.required),
                                'source_revision': self.source.source_revision})
            self.request_snapshots(nonce, frozenset(self.required))
            if self.active and self.nonce == nonce and not self.required:
                self._finish()
        except Exception:
            if self.nonce == nonce:
                self._discard()
            raise

    def acquire(self, key):
        """Called before native exporter allocations, on the source thread."""
        if not self.active or self.closed or key not in self.required:
            return None
        if self.clock() >= self.deadline:
            self.cancel('capture-deadline')
            raise BarrierRejected('capture-deadline')
        try:
            _, current = self._read_boundary()
        except Exception:
            self.cancel("native-boundary-read-failed")
            raise
        if current != self.boundary:
            self.invalidate('native-boundary-changed-before-snapshot')
            return None
        # At most one retained candidate per surface. Further callbacks belong
        # to a later native frame and do not replace an earlier candidate.
        return self.tickets.get(key) if key not in self.candidates else None

    def submit(self, ticket, capture, frame, offers):
        if ticket is None:
            return False
        if ticket.owner is not self or ticket.finished or ticket.nonce != self.nonce or not self.active:
            ticket.close()
            return False
        if self.registered.get(ticket.key, (None,))[0] is not capture:
            self.cancel('unknown-native-participant')
            raise BarrierRejected('unknown-native-participant')
        try:
            facts, current = self._read_boundary()
        except Exception:
            self.cancel("native-boundary-read-failed")
            raise
        if current != self.boundary or frame['source_revision'] != self.source.source_revision:
            self.invalidate('native-boundary-changed-during-snapshot')
            return False
        if frame['capture_boundary'] != 'actual-snapshot-vfunc-same-node-append' or frame.get('capture_barrier_nonce') != ticket.nonce:
            self.cancel('missing-fresh-native-capture-proof')
            raise BarrierRejected('missing-fresh-native-capture-proof')
        if len(frame['scene']) + len(frame['semantics']) > MAX_RECORDS or len(offers) > MAX_RESOURCES:
            self.cancel('capture-record-or-resource-ceiling')
            raise BarrierRejected('capture-record-or-resource-ceiling')
        if frame["semantics"] != facts["surfaces"][ticket.key]["semantics"]:
            self.invalidate("captured-semantic-payload-differs-from-boundary")
            return False
        try:
            # Resource descriptors come from the actual source write. Charge
            # new union bodies before retaining descriptors/capture ownership.
            for offer in offers:
                digest = offer['digest']
                size = offer['total_bytes']
                if (not isinstance(digest, str) or len(digest) != 64
                        or any(c not in '0123456789abcdef' for c in digest)
                        or type(size) is not int or not 0 < size <= 16 << 20
                        or offer['kind'] not in (1, 2) or offer['provenance'] != offer['kind']
                        or offer['version'] != 1):
                    raise BarrierRejected('invalid-source-resource-descriptor')
                if digest in self.pending_union:
                    if self.pending_union[digest] != offer:
                        raise BarrierRejected('conflicting-source-descriptor')
                    continue
                total = self.union_bytes + size
                if len(self.pending_union) >= MAX_RESOURCES or total > MAX_UNION_BYTES:
                    raise BarrierRejected('capture-union-ceiling')
                reservation = self.budget.reserve(offer['total_bytes'] + 1024)
                self.body_reservations[digest] = reservation
                self.pending_union[digest] = offer
                self.union_bytes = total
            self.candidates[ticket.key] = frame
            self.events.append({'event': 'fresh-capture', 'nonce': ticket.nonce, 'surface': ticket.key,
                                'token': frame['capture_token']})
            if self.on_candidate:
                self.on_candidate(ticket.nonce, ticket.key)
            if self.active and ticket.nonce == self.nonce and set(self.candidates) == self.required:
                self._finish()
            return True
        except Exception:
            if ticket.nonce == self.nonce:
                self._discard()
            raise

    def _finish(self):
        if self.clock() >= self.deadline:
            self.cancel('capture-deadline-at-commit')
            raise BarrierRejected('capture-deadline-at-commit')
        try:
            facts, current = self._read_boundary()
        except Exception:
            self.cancel("native-boundary-read-failed-at-commit")
            raise
        if current != self.boundary:
            self.invalidate('semantic-focus-topology-or-geometry-changed-at-commit')
            return
        if set(self.candidates) != self.required:
            raise BarrierRejected('incomplete-source-transaction')
        self.encoding_reservation = self.budget.reserve(METADATA_CHARGE)
        # The source callback must synchronously hand off or reject this union;
        # it cannot retain borrowed candidate frames beyond this transaction.
        self.committing = True
        try:
            self.committed(self.nonce, facts, self.candidates, self.pending_union)
        finally:
            self.committing = False
        self.events.append({'event': 'commit', 'nonce': self.nonce, 'surfaces': sorted(self.required)})
        self._discard()

    def invalidate(self, reason):
        if not self.active:
            return
        if self.committing:
            self.cancel("mutation-during-serialized-handoff")
            raise BarrierRejected("mutation-during-serialized-handoff")
        self.events.append({'event': 'abort', 'nonce': self.nonce, 'reason': reason})
        self._discard()
        if self.clock() >= self.deadline or self.restarts >= self.max_restarts:
            raise BarrierRejected('capture-restart-or-deadline-ceiling')
        self.restarts += 1
        self._start()

    def expire(self):
        """Source actor calls from its bounded deadline timer, even without frames."""
        if self.active and self.clock() >= self.deadline:
            self.cancel("capture-deadline-without-frame")
            return True
        return False

    def cancel(self, reason='capture-cancelled'):
        if self.active:
            self.events.append({'event': 'cancel', 'nonce': self.nonce, 'reason': reason})
        self._discard()

    def _discard(self):
        for ticket in self.tickets.values():
            ticket.close()
        for reservation in self.body_reservations.values():
            reservation.close()
        for reservation in (self.boundary_reservation, self.encoding_reservation):
            if reservation:
                reservation.close()
        self.tickets.clear()
        self.candidates.clear()
        self.pending_union.clear()
        self.union_bytes = 0
        self.body_reservations.clear()
        self.boundary_reservation = self.encoding_reservation = None
        self.boundary = None
        self.required.clear()
        self.active = False

    def close(self):
        self.cancel('source-disposed')
        for capture, reservation in self.registered.values():
            # These are explicitly coordinator-owned retained frame slots.
            if hasattr(capture, "last_frame"):
                capture.last_frame = None
            if hasattr(capture, "captures"):
                capture.captures.clear()
            reservation.close()
        self.registered.clear()
        self.events.clear()
        self.metadata_reservation.close()
        self.closed = True
