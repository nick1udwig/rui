import hashlib
import struct
import unittest

from conformance.semantic_client import (
    ProtocolError, PinnedWebSocket, SemanticClient, SemanticReplica,
    blob, decode, scalar, value,
)


def ref(kind, identity):
    return scalar(1, kind) + scalar(2, identity)


def node(identity, version=1, role=3, label='Increment', children=(), extra=b''):
    body = scalar(1, identity) + scalar(2, version) + scalar(3, role) + blob(4, label)
    body += b''.join(blob(8, ref(3, child)) for child in children)
    return blob(3, body + scalar(16, 1) + scalar(17, 1) + extra)


def snapshot(records, revision=1):
    return scalar(1, revision) + b''.join(blob(2, record) for record in records)


def fixed(tag, number):
    return bytes([(tag << 3) | 1]) + struct.pack('<d', number)


def surface(identity, root, extra=b''):
    rectangle = fixed(3, 100) + fixed(4, 100)
    body = (scalar(1, identity) + scalar(2, 1) + scalar(3, 1) + blob(5, rectangle)
            + blob(6, fixed(1, 100) + fixed(2, 100)) + blob(7, fixed(1, 1) + fixed(4, 1))
            + fixed(9, 1) + scalar(11, 1) + blob(15, ref(3, root))
            + scalar(18, 3) + scalar(19, 1) + scalar(20, 1)
            + blob(21, 'source-commit-1') + blob(22, 'semantic-only'))
    return blob(1, body + extra)


class ReplicaTests(unittest.TestCase):
    def test_negotiated_record_and_depth_limits_apply_atomically(self):
        for replica in (SemanticReplica(record_limit=1), SemanticReplica(depth_limit=1)):
            with self.assertRaises(ProtocolError):
                replica.apply(1, snapshot([node(1, children=(2,)), node(2)]))
            self.assertEqual(replica.revision, 0)
        replica = SemanticReplica(record_limit=1, depth_limit=1)
        replica.apply(1, snapshot([node(1)]))
        with self.assertRaises(ProtocolError):
            replica.apply(2, scalar(1, 1) + scalar(2, 2) + blob(3, blob(1, node(2))))
        self.assertEqual(set(replica.records), {1})

    def test_default_scalar_encoding_does_not_change_unchanged_record(self):
        replica = SemanticReplica()
        original = blob(4, scalar(1, 10) + scalar(2, 1) + scalar(3, 1) + scalar(4, 1) + blob(5, 'text'))
        explicit = blob(4, scalar(1, 10) + scalar(2, 1) + scalar(3, 1) + scalar(4, 1) + blob(5, 'text') + scalar(7, 0))
        replica.apply(1, snapshot([original]))
        replica.apply(1, snapshot([explicit], 2))
        # Optional reading-order zero has presence and is a real record change.
        replica = SemanticReplica()
        replica.apply(1, snapshot([node(1)]))
        with self.assertRaises(ProtocolError):
            replica.apply(1, snapshot([node(1, extra=scalar(15, 0))], 2))

    def test_review_regressions_typed_clocks_and_optional_metadata(self):
        from conformance.typed_interop import text_model
        replica = SemanticReplica()
        replica.apply(1, snapshot([node(1), text_model()]))
        for changed in (text_model(2, 3, 'changed', 0, 0), text_model(2, 2)):
            with self.assertRaises(ProtocolError):
                replica.apply(2, scalar(1, 1) + scalar(2, 2) + blob(3, blob(1, changed)))
        # Optional metadata changes neither known behavior nor record version.
        replica.apply(1, snapshot([node(1, extra=scalar(999, 9)), text_model()], 2))
        self.assertEqual(replica.revision, 2)

    def test_review_regressions_text_collection_and_relation_constraints(self):
        unselectable = blob(4, scalar(1, 10) + scalar(2, 1) + scalar(3, 1) + scalar(4, 1)
                            + blob(5, 'text') + blob(6, scalar(1, 1)))
        editable_unavailable = blob(4, scalar(1, 10) + scalar(2, 1) + scalar(3, 1) + scalar(4, 2) + scalar(7, 1))
        foreign_item = node(1, extra=blob(12, scalar(1, 1) + blob(2, ref(3, 2)) + scalar(4, 2)))
        wrong_relation = node(1, extra=blob(9, scalar(1, 1) + blob(2, ref(4, 10))))
        for records in ([unselectable], [editable_unavailable], [foreign_item, node(2)], [wrong_relation]):
            replica = SemanticReplica()
            with self.assertRaises(ProtocolError):
                replica.apply(1, snapshot(records))
            self.assertEqual(replica.revision, 0)

    def test_review_regressions_surface_root_and_focus_ownership(self):
        valid = [surface(100, 1), node(1)]
        replica = SemanticReplica()
        replica.apply(1, snapshot(valid))
        session = blob(6, scalar(1, 200) + scalar(2, 1) + blob(3, fixed(3, 100) + fixed(4, 100))
                       + blob(5, ref(3, 1)) + blob(6, ref(1, 101)) + scalar(7, 1))
        for records in (valid + [node(2)], valid + [surface(101, 1)],
                        valid + [surface(101, 2), node(2), session],
                        [blob(1, scalar(1, 100) + scalar(2, 1) + scalar(18, 3))]):
            replica = SemanticReplica()
            with self.assertRaises(ProtocolError):
                replica.apply(1, snapshot(records))
            self.assertEqual(replica.revision, 0)

    def test_review_regressions_redaction_and_capture_evidence(self):
        redacted = blob(3, scalar(1, 1) + scalar(2, 1) + scalar(3, 3)
                        + blob(4, 'private') + scalar(16, 3) + scalar(17, 1))
        for records in ([redacted], [surface(100, 1, blob(21, b'')), node(1)]):
            with self.assertRaises(ProtocolError):
                SemanticReplica().apply(1, snapshot(records))

    def test_discovery_unicode_scalar_selection_and_collection_without_coordinates(self):
        text = blob(4, scalar(1, 20) + scalar(2, 1) + scalar(3, 4) + scalar(4, 1)
                    + blob(5, 'Aé🦀Z') + blob(6, scalar(1, 1) + scalar(2, 3)) + scalar(7, 1) + scalar(8, 1))
        action = blob(10, blob(1, 'ReplaceText') + scalar(2, 1))
        edit = node(2, role=5, label='Public text', extra=blob(11, ref(4, 20)) + action)
        collection = scalar(1, 7) + blob(2, ref(3, 4)) + blob(2, ref(3, 5)) + scalar(3, 10) + scalar(4, 2) + blob(5, ref(3, 5))
        listing = node(3, role=7, label='Items', children=(4, 5), extra=blob(12, collection))
        replica = SemanticReplica()
        replica.apply(1, snapshot([node(1, role=1, children=(2, 3)), edit, listing, node(4, role=8), node(5, role=8), text]))
        self.assertEqual(replica.discover(action='ReplaceText')[0]['id'], 2)
        self.assertEqual(replica.records[20]['selection'], (1, 3))
        self.assertEqual(replica.records[3]['collection']['selected'], [5])
        reorder = scalar(1, 8) + blob(2, ref(3, 5)) + blob(2, ref(3, 4)) + scalar(3, 10) + scalar(4, 2)
        update = node(3, version=2, role=7, label='Items', children=(5, 4), extra=blob(12, reorder))
        replica.apply(2, scalar(1, 1) + scalar(2, 2) + blob(3, blob(1, update)))
        self.assertEqual(replica.records[3]['collection']['items'], [5, 4])

    def test_atomic_dangling_rejection_retirement_and_version_rejection(self):
        replica = SemanticReplica()
        replica.apply(1, snapshot([node(1, children=(2,)), node(2)]))
        original = replica.records.copy()
        delete = scalar(1, 1) + scalar(2, 2) + blob(3, blob(2, ref(3, 2)))
        with self.assertRaises(ProtocolError):
            replica.apply(2, delete)
        self.assertEqual(replica.records, original)
        self.assertEqual(replica.revision, 1)
        replica.apply(2, delete + blob(3, blob(1, node(1, version=2))))
        with self.assertRaises(ProtocolError):
            replica.apply(2, scalar(1, 2) + scalar(2, 3) + blob(3, blob(1, node(2))))
        self.assertEqual(replica.retired, {2})
        with self.assertRaises(ProtocolError):
            replica.apply(2, scalar(1, 2) + scalar(2, 3) + blob(3, blob(1, node(1, version=4))))

    def test_wrong_kind_cycles_resources_and_required_vocabulary_reject(self):
        for payload in (snapshot([node(1, children=(1,))]),
                        snapshot([node(1)]) + blob(3, bytes(32)),
                        snapshot([blob(2, scalar(1, 1) + scalar(2, 1))]),
                        snapshot([node(1)]) + scalar(100, 1)):
            replica = SemanticReplica()
            with self.assertRaises(ProtocolError):
                replica.apply(1, payload)
            self.assertEqual(replica.revision, 0)
        self.assertEqual(value(decode(scalar(1, 4) + scalar(999, 7), {1}), 1), 4)
        with self.assertRaises(ProtocolError):
            value(decode(scalar(1, 4) + scalar(1, 5), {1}), 1)


class MemoryWire:
    def __init__(self, incoming):
        self.incoming = iter(incoming)
        self.sent = []
        self.ceiling = 65536

    def receive(self):
        return next(self.incoming)

    def send(self, data):
        self.sent.append(data)


def client_fixture(messages):
    client = SemanticClient.__new__(SemanticClient)
    client.wire = MemoryWire(messages)
    client.binding = (bytes([1]) * 16, bytes([2]) * 16, bytes([3]) * 16)
    client.instance_id = bytes([4]) * 16
    client.limits = client.LIMITS
    client.pending = None
    client.replica = SemanticReplica()
    client.lease = (bytes([5]) * 16, 1)
    client.sequence = 0
    client.last_command = client.last_result = None
    return client


def packet(kind, payload, view=3):
    return b''.join(blob(tag, bytes([identity]) * 16) for tag, identity in ((1, 1), (2, 2), (3, view))) + scalar(4, 1) + scalar(5, kind) + blob(6, payload)


class ClientTests(unittest.TestCase):
    def test_exact_publication_then_applied_and_semantic_ready_never_presented(self):
        body = snapshot([node(1, extra=blob(10, blob(1, 'Activate') + scalar(2, 1)))])
        publication = bytes([9]) * 16
        digest = hashlib.sha256(body).digest()
        begin = blob(1, publication) + scalar(3, 1) + scalar(4, len(body)) + scalar(5, 1) + scalar(6, 1) + blob(7, digest)
        chunk = blob(1, publication) + blob(3, body)
        commit = blob(1, publication) + scalar(2, len(body)) + scalar(3, 1) + scalar(4, 1) + blob(5, digest)
        client = client_fixture([packet(10, begin), packet(12, chunk), packet(12, chunk), packet(13, commit)])
        self.assertEqual(client.receive(), ('applied', 1))
        self.assertEqual([value(decode(raw, set(range(1, 8))), 5) for raw in client.wire.sent], [30, 35])
        client.command(1, 'Activate')
        original = client.wire.sent[-1]
        client.retry_last()
        self.assertEqual(original, client.wire.sent[-1])
        self.assertEqual(client.sequence, 1)

    def test_binding_or_digest_conflicts_never_apply(self):
        client = client_fixture([packet(6, scalar(1, 1), view=8)])
        with self.assertRaises(ProtocolError):
            client.receive()
        self.assertEqual(client.wire.sent, [])
        self.assertEqual(client.replica.revision, 0)

    def test_applied_requires_source_evidence_and_exact_result_sequence(self):
        for result in (scalar(1, 1) + blob(2, 'Applied'), scalar(1, 2) + blob(2, 'Dispatched')):
            client = client_fixture([packet(54, result)])
            client.sequence = 1
            with self.assertRaises(ProtocolError):
                client.receive()

    def test_websocket_rejects_oversized_length_before_reading_payload(self):
        ws = PinnedWebSocket.__new__(PinnedWebSocket)
        ws.ceiling = 512
        reads = iter((b'\x82\x7f', struct.pack('!Q', 1 << 40)))
        ws._read = lambda count: next(reads)
        with self.assertRaises(ProtocolError):
            ws.receive()


if __name__ == '__main__':
    unittest.main()
