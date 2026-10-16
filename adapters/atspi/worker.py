"""Disposable, bounded AT-SPI reader. No native handle leaves the private IPC lane.

Every blocking accessibility call lives in this process. The publisher polls the
socket and can kill/restart this worker without blocking its own publication loop.
"""
import argparse
import json
import os
import resource
import select
import socket
import time

MAX_MESSAGE = 128 << 10
MAX_NODES = 128
MAX_DEPTH = 16
MAX_CALLS = 1024
MAX_STRING = 256
CALL_TIMEOUT_MS = 200
CRAWL_SECONDS = 2.0


def bounded_string(value):
    if not isinstance(value, str) or len(value.encode('utf-8')) > MAX_STRING:
        raise ValueError('accessibility string ceiling')
    return value


def identity(value):
    if not isinstance(value, (tuple, list)) or len(value) != 2:
        raise ValueError('invalid accessible identity')
    bus, path = map(bounded_string, value)
    if not bus.startswith(':') or not path.startswith('/') or path.endswith('/null'):
        raise ValueError('unresolved accessible identity')
    return bus, path


class Reader:
    max_calls = MAX_CALLS
    call_timeout_ms = CALL_TIMEOUT_MS

    def __init__(self, channel, address, observe_events=True):
        import gi
        gi.require_version('Gio', '2.0')
        from gi.repository import Gio, GLib
        self.Gio, self.GLib = Gio, GLib
        self.channel = channel
        self.bus = Gio.DBusConnection.new_for_address_sync(
            address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
        self.context = GLib.MainContext.default()
        self.events = 0
        self.dropped = 0
        self.suppress_events = False
        self.generation = 0
        self.exhausted = False
        self.selected = set()
        self.registered = set()
        if observe_events:
            self.bus.signal_subscribe(None, 'org.a11y.atspi.Event.Object', None, None, None,
                                      Gio.DBusSignalFlags.NONE, self.event, None)
        self.calls = 0
        self.deadline = 0
        self.hook = False

    def event(self, connection, sender, path, interface, signal, parameters, data):
        if sender not in self.selected:
            return
        self.events = min(self.events + 1, (1 << 63) - 1)
        if self.suppress_events:
            self.dropped = min(self.dropped + 1, (1 << 63) - 1)
        else:
            if self.generation == (1 << 63) - 1:
                self.exhausted = True
            else:
                self.generation += 1

    def pump(self):
        if self.exhausted:
            raise ValueError('accessibility event identity exhausted')
        for _ in range(256):
            if not self.context.pending():
                return
            self.context.iteration(False)
        raise ValueError('accessibility event storm')

    def call(self, key, interface, method, args=None):
        self.calls += 1
        if self.calls > self.max_calls or time.monotonic() >= self.deadline:
            raise ValueError('accessibility crawl deadline or call ceiling')
        result = self.bus.call_sync(key[0], key[1], interface, method, args, None,
                                    self.Gio.DBusCallFlags.NONE, self.call_timeout_ms, None)
        if result.get_size() > MAX_MESSAGE:
            raise ValueError('accessibility reply ceiling')
        self.pump()
        return result.unpack()

    def children(self, key):
        children = self.call(key, 'org.a11y.atspi.Accessible', 'GetChildren')[0]
        if len(children) > MAX_NODES:
            raise ValueError('child ceiling')
        result = [identity(child) for child in children]
        if len(set(result)) != len(result) or any(child[0] != key[0] for child in result):
            raise ValueError('duplicate or cross-application children')
        return result

    def barrier(self):
        if not self.hook:
            return
        self.hook = False
        self.channel.send(json.dumps({'type': 'crawl_barrier', 'request_id': self.request_id}).encode())
        ready, _, _ = select.select([self.channel], [], [], .75)
        if not ready or self.channel.recv(64) != b'continue':
            raise ValueError('fixture crawl barrier expired')
        self.pump()

    def tree(self, key, parent, records, depth):
        if depth > MAX_DEPTH or len(records) >= MAX_NODES or key in records:
            raise ValueError('cycle, duplicate ownership or tree ceiling')
        properties = self.call(key, 'org.freedesktop.DBus.Properties', 'GetAll',
                               self.GLib.Variant('(s)', ('org.a11y.atspi.Accessible',)))[0]
        if tuple(properties.get('Parent', ())) != parent:
            raise ValueError('parent changed during traversal')
        children = self.children(key)
        if properties.get('ChildCount') != len(children):
            raise ValueError('children changed during traversal')
        role = self.call(key, 'org.a11y.atspi.Accessible', 'GetRole')[0]
        # No text/value/action interface is read. Password-role labels are redacted.
        name = None if role == 40 else bounded_string(properties.get('Name', ''))
        records[key] = {'key': key, 'parent': parent, 'children': children, 'name': name, 'role': role}
        self.barrier()
        for child in children:
            self.tree(child, key, records, depth + 1)
        if self.children(key) != children:
            raise ValueError('children changed after traversal')

    def sample(self, targets):
        roots = self.call(('org.a11y.atspi.Registry', '/org/a11y/atspi/accessible/root'),
                          'org.a11y.atspi.Accessible', 'GetChildren')[0]
        if len(roots) > 16:
            raise ValueError('application ceiling')
        applications = []
        owned = []
        for raw in roots:
            root = identity(raw)
            pid = self.call(('org.freedesktop.DBus', '/org/freedesktop/DBus'),
                            'org.freedesktop.DBus', 'GetConnectionUnixProcessID',
                            self.GLib.Variant('(s)', (root[0],)))[0]
            if pid not in targets:
                continue
            owned.append((root, pid))
        if {pid for _, pid in owned} != set(targets) or len(owned) != len(targets):
            raise ValueError('owned application absent or ambiguous')
        selected = {root[0] for root, _ in owned}
        # Reconcile subscriptions before walking either application. Failed
        # crawls cannot accumulate registrations for old process incarnations.
        self.selected = selected
        for old in list(self.registered - selected):
            for event in ('object:children-changed', 'object:property-change'):
                self.call(('org.a11y.atspi.Registry', '/org/a11y/atspi/registry'),
                          'org.a11y.atspi.Registry', 'DeregisterEvent',
                          self.GLib.Variant('(ss)', (event, old)))
            self.registered.remove(old)
        for root, pid in owned:
            if root[0] not in self.registered:
                self.registered.add(root[0])
                for event in ('object:children-changed', 'object:property-change'):
                    self.call(('org.a11y.atspi.Registry', '/org/a11y/atspi/registry'),
                              'org.a11y.atspi.Registry', 'RegisterEvent',
                              self.GLib.Variant('(sass)', (event, [], root[0])))
            frames = []
            children = self.children(root)
            if len(children) > 4:
                raise ValueError('window ceiling')
            for key in children:
                records = {}
                self.tree(key, root, records, 0)
                role = records[key]['role']
                if role not in (23, 69):
                    raise ValueError('unresolved top-level accessible')
                bounds = self.call(key, 'org.a11y.atspi.Component', 'GetExtents',
                                   self.GLib.Variant('(u)', (0,)))[0]
                if len(bounds) != 4 or any(abs(v) > 16384 for v in bounds) or min(bounds[2:]) <= 0:
                    raise ValueError('unavailable observed window geometry')
                frames.append({'root': key, 'bounds': bounds, 'records': list(records.values())})
            if len(frames) > 4:
                raise ValueError('window ceiling')
            applications.append({'pid': pid, 'root': root, 'frames': frames})
        return applications

    def crawl(self, request):
        self.request_id = request['request_id']
        targets = request['pids']
        if (not isinstance(targets, list) or len(targets) > 2
                or len(set(targets)) != len(targets)
                or any(type(pid) is not int or pid <= 0 for pid in targets)):
            raise ValueError('owned application scope required')
        self.calls = 0
        self.deadline = time.monotonic() + CRAWL_SECONDS
        self.hook = request.get('fixture_barrier') is True
        failures = 0
        for _ in range(2):
            try:
                self.pump()
                before = self.generation
                first = self.sample(targets)
                second = self.sample(targets)
                self.pump()
                if before != self.generation or first != second:
                    raise ValueError('observed subtree changed during crawl')
                return {'type': 'snapshot', 'apps': second, 'classification': 'OBSERVED',
                        'reconciliations': failures, 'calls': self.calls,
                        'events': self.events, 'dropped_events': self.dropped,
                        'worker_peak_rss_kib': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}
            except Exception:
                failures += 1
                if time.monotonic() >= self.deadline:
                    break
        return {'type': 'unavailable', 'reason': 'bounded-reconciliation-failed',
                'reconciliations': failures, 'calls': self.calls}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--fd', type=int, required=True)
    args = parser.parse_args()
    # Local readers can allocate opaque GLib replies before their size is known.
    # The disposable process has a hard address-space ceiling in addition to
    # explicit message/tree limits; the publisher never inherits those replies.
    resource.setrlimit(resource.RLIMIT_AS, (256 << 20, 256 << 20))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    channel = socket.socket(fileno=args.fd)
    address = os.environ.get('AT_SPI_BUS_ADDRESS')
    if not address or len(address) > 1024:
        raise ValueError('explicit accessibility bus required')
    reader = Reader(channel, address)
    while True:
        reader.pump()
        ready, _, _ = select.select([channel], [], [], .01)
        if not ready:
            continue
        raw = channel.recv(4097)
        if not raw:
            return
        if len(raw) > 4096:
            raise ValueError('request ceiling')
        request = json.loads(raw)
        if request.get('op') == 'suppress_fixture_events':
            reader.suppress_events = bool(request['value'])
            result = {'type': 'configured'}
        elif request.get('op') == 'crawl':
            result = reader.crawl(request)
        else:
            raise ValueError('unknown trusted worker operation')
        result['request_id'] = request['request_id']
        payload = json.dumps(result, separators=(',', ':')).encode()
        if len(payload) > MAX_MESSAGE:
            payload = json.dumps({'type':'unavailable','reason':'message-ceiling','request_id':request['request_id']}).encode()
        channel.send(payload)


if __name__ == '__main__':
    main()
