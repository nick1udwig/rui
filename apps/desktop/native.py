#!/usr/bin/python3
"""Bounded, disposable AT-SPI jobs for explicitly approved existing applications.

Native references stay on the private owner lane. No coordinate input, scripts,
clipboard access, or compositor association. Mutations have observed semantics:
revalidation narrows races but AT-SPI has no atomic compare-and-set operation.
"""
import hashlib
import json
import os
from pathlib import Path
import resource
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from adapters.atspi.worker import Reader, identity

A = 'org.a11y.atspi.'
MAX_TEXT = 16384
MAX_NODES = 512
ACTION_ROLES = (7, 8, 11, 32, 35, 37, 43, 44, 45, 56, 62, 88, 91)


def text(value, limit=1024):
    if not isinstance(value, str) or len(value.encode('utf-8')) > limit:
        raise ValueError('text ceiling')
    return value


def process_stamp(pid):
    directory = Path('/proc') / str(pid)
    if directory.stat().st_uid != os.getuid():
        raise ValueError('application belongs to another user')
    # comm may contain spaces and parentheses; starttime is field 22.
    return (directory / 'stat').read_text().rsplit(')', 1)[1].split()[19]


class DesktopReader(Reader):
    max_calls = 8192
    call_timeout_ms = 400

    def __init__(self):
        import gi
        gi.require_version('Gio', '2.0')
        from gi.repository import Gio
        address = os.environ.get('AT_SPI_BUS_ADDRESS')
        if not address:
            session = Gio.bus_get_sync(Gio.BusType.SESSION, None)
            address = session.call_sync('org.a11y.Bus', '/org/a11y/bus', 'org.a11y.Bus',
                                        'GetAddress', None, None, Gio.DBusCallFlags.NONE,
                                        1000, None).unpack()[0]
        if not address or len(address) > 1024:
            raise ValueError('accessibility bus unavailable')
        super().__init__(None, address, observe_events=False)
        self.deadline = time.monotonic() + 1.8

    def props(self, key, interface):
        return self.call(key, 'org.freedesktop.DBus.Properties', 'GetAll',
                         self.GLib.Variant('(s)', (A + interface,)))[0]

    def pid(self, key):
        return self.call(('org.freedesktop.DBus', '/org/freedesktop/DBus'),
                         'org.freedesktop.DBus', 'GetConnectionUnixProcessID',
                         self.GLib.Variant('(s)', (key[0],)))[0]

    def roots(self):
        roots = self.call(('org.a11y.atspi.Registry', '/org/a11y/atspi/accessible/root'),
                          A + 'Accessible', 'GetChildren')[0]
        if len(roots) > 64:
            raise ValueError('application ceiling')
        return [identity(k) for k in roots]

    def discover(self):
        apps = []
        for key in self.roots():
            try:
                pid = self.pid(key)
                stamp = process_stamp(pid)
                name = text(self.props(key, 'Accessible').get('Name', ''))
                apps.append({'root': key, 'pid': pid, 'start': stamp, 'name': name or f'Application {pid}'})
            except Exception:
                continue  # One inaccessible app must not hide the others.
        return {'apps': apps}

    def verify(self, source):
        key = identity(source['root'])
        if (key not in self.roots() or self.pid(key) != source['pid']
                or process_stamp(source['pid']) != source['start']):
            raise ValueError('application exited or changed identity')
        return key

    def batch(self, requests):
        """Bounded asynchronous D-Bus batches, with no source-side code execution."""
        results = []
        for offset in range(0, len(requests), 32):
            chunk = requests[offset:offset + 32]
            self.calls += len(chunk)
            if self.calls > self.max_calls or time.monotonic() >= self.deadline:
                raise ValueError('accessibility crawl deadline or call ceiling')
            pending = len(chunk)
            values, errors = [None] * pending, []
            cancel = self.Gio.Cancellable()

            def completed(connection, result, index):
                nonlocal pending
                try:
                    reply = connection.call_finish(result)
                    if reply.get_size() > 128 << 10:
                        raise ValueError('accessibility reply ceiling')
                    values[index] = reply.unpack()
                except Exception as error:
                    errors.append(error)
                    cancel.cancel()
                finally:
                    pending -= 1

            for index, (key, interface, method, args) in enumerate(chunk):
                self.bus.call(key[0], key[1], interface, method, args, None,
                              self.Gio.DBusCallFlags.NONE, self.call_timeout_ms,
                              cancel, completed, index)
            while pending:
                self.context.iteration(True)
                if time.monotonic() >= self.deadline:
                    cancel.cancel()
            if errors:
                raise errors[0]
            results.extend(values)
        return results

    def sample(self, source):
        root = self.verify(source)
        nodes, metadata, visited = [], {}, set()
        frontier = [(root, None)]
        depth = 0
        while frontier:
            if depth > 24 or len(visited) + len(frontier) > 1024:
                raise ValueError('application tree ceiling')
            for key, parent in frontier:
                if key in visited or key[0] != root[0]:
                    raise ValueError('application cycle or cross-application reference')
                visited.add(key)
            states = self.batch([(key, A + 'Accessible', 'GetState', None)
                                 for key, _ in frontier])
            visible = []
            for (key, parent), (words,) in zip(frontier, states):
                bits = sum(int(word) << (32 * i) for i, word in enumerate(words))
                if bits & (1 << 6):
                    raise ValueError('defunct object')
                if key == root or bits & (1 << 25) and bits & (1 << 30):
                    visible.append((key, parent, bits))
            roles = self.batch([(key, A + 'Accessible', 'GetRole', None)
                                for key, _, _ in visible])
            if len(nodes) + len(visible) > MAX_NODES:
                raise ValueError('visible application tree ceiling')
            readable, requests = [], []
            for (key, parent, bits), (role,) in zip(visible, roles):
                # Never query password names, text, descriptions or descendants.
                if role == 40:
                    nodes.append({'key': key, 'parent': parent, 'role': 'label',
                                  'label': 'Password field — unavailable', 'children': [],
                                  'actions': [], 'disabled': True})
                    continue
                readable.append((key, parent, bits, role))
                requests.extend([(key, 'org.freedesktop.DBus.Properties', 'GetAll',
                                  self.GLib.Variant('(s)', (A + 'Accessible',))),
                                 (key, A + 'Accessible', 'GetInterfaces', None)])
            answers = self.batch(requests)
            parents, next_frontier = [], []
            for index, (key, parent, bits, role) in enumerate(readable):
                props, interfaces = answers[index * 2][0], answers[index * 2 + 1][0]
                if key != root and tuple(props.get('Parent', ())) != parent:
                    raise ValueError('parent changed')
                count = props.get('ChildCount')
                if type(count) is not int or count < 0 or count > 128:
                    raise ValueError('child ceiling')
                has = lambda bit: bool(bits & (1 << bit))
                # GTK4 uses SENSITIVE without ENABLED; Writer paragraphs use
                # ENABLED without SENSITIVE. Text also requires EDITABLE below.
                node = {'key': key, 'parent': parent, 'role': 'group' if count else 'label',
                        'label': text(props.get('Name', '')), 'children': [], 'actions': [],
                        'disabled': not ((has(8) or has(24)) and has(25) and has(30)),
                        'modal': has(16), 'window': role in (16, 23, 69)}
                nodes.append(node)
                metadata[key] = (node, role, bits, interfaces)
                if count:
                    parents.append((key, count, node))
            children = self.batch([(key, A + 'Accessible', 'GetChildren', None)
                                   for key, _, _ in parents])
            for (key, count, node), (refs,) in zip(parents, children):
                if len(refs) != count:
                    raise ValueError('children changed')
                node['children'] = [identity(ref) for ref in refs]
                next_frontier.extend((child, key) for child in node['children'])
            frontier, depth = next_frontier, depth + 1

        # Query text and default-action metadata in batches as well. Reading
        # ordinary label names above avoids redundant GetText calls on labels.
        requests, operations = [], []
        for key, (node, role, bits, interfaces) in metadata.items():
            if A + 'Text' in interfaces and (role != 29 or not node['label']):
                requests.append((key, 'org.freedesktop.DBus.Properties', 'GetAll',
                                 self.GLib.Variant('(s)', (A + 'Text',))))
                operations.append(('text', key))
            if A + 'Action' in interfaces and role in ACTION_ROLES:
                requests.append((key, A + 'Action', 'GetActions', None))
                operations.append(('action', key))
        text_requests, text_keys = [], []
        for (operation, key), (answer,) in zip(operations, self.batch(requests)):
            node, role, bits, interfaces = metadata[key]
            if operation == 'action':
                if answer:
                    node.update(role='button', native_action=text(answer[0][0]))
                    if not node['disabled']:
                        node['actions'] = ['Activate']
                    if role in (7, 44):
                        node['description'] = 'Checked' if bits & (1 << 4) else 'Unchecked'
            else:
                count = answer['CharacterCount']
                if type(count) is not int or count < 0:
                    raise ValueError('invalid text length')
                if count <= MAX_TEXT:
                    text_keys.append(key)
                    text_requests.append((key, A + 'Text', 'GetText',
                                          self.GLib.Variant('(ii)', (0, count))))
                else:
                    node['description'] = 'Text exceeds the 16 KiB limit; editing unavailable.'
        for key, (value,) in zip(text_keys, self.batch(text_requests)):
            node, role, bits, interfaces = metadata[key]
            if value and set(value) <= {'●', '•', '*', '▪', '■'}:
                # Some GTK entries mask text but omit PASSWORD_TEXT.
                node.update(role='label', label='Protected field — unavailable',
                            disabled=True, actions=[])
            elif len(value.encode('utf-8')) <= MAX_TEXT:
                if A + 'EditableText' in interfaces or role in (61, 79):
                    node.update(role='text', kind='textarea' if bits & (1 << 17) else 'text', value=value)
                elif value != node['label']:
                    node['description'] = value
                if bits & (1 << 7) and not bits & (1 << 43) and A + 'EditableText' in interfaces and not node['disabled']:
                    node['actions'] = ['SetValue']
            else:
                node['description'] = 'Text exceeds the 16 KiB limit; editing unavailable.'
        branches = [node for node in nodes if node['children']]
        answers = self.batch([(node['key'], A + 'Accessible', 'GetChildren', None)
                              for node in branches])
        visible_keys = {node['key'] for node in nodes}
        for node, (refs,) in zip(branches, answers):
            if [identity(ref) for ref in refs] != node['children']:
                raise ValueError('children changed during observation')
            node['children'] = [key for key in node['children'] if key in visible_keys]
        # A modal dialog blocks all other application subtrees. Multiple modal
        # roots are ambiguous; expose them read-only until the source resolves it.
        modal = [n['key'] for n in nodes if n.get('modal')]
        by_key = {n['key']: n for n in nodes}
        for node in nodes:
            if modal:
                k = node['key']
                ancestry = []
                while k is not None:
                    ancestry.append(k)
                    k = by_key[k]['parent']
                if len(modal) != 1 or modal[0] not in ancestry:
                    node['actions'] = []
                    node['disabled'] = True
        # Revalidate the target, its ownership chain and all active modals.
        # Unrelated status bars (word counts, selection hints) must not make an
        # otherwise unchanged Save button unusable in real applications.
        for node in nodes:
            ancestry = []
            key = node['parent']
            while key is not None:
                parent = by_key[key]
                ancestry.append({field: parent.get(field) for field in
                                 ('key', 'parent', 'children', 'disabled', 'modal')})
                key = parent['parent']
            node['fingerprint'] = hashlib.sha256(json.dumps(
                [node, ancestry, modal], sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        self.verify(source)
        fingerprint = hashlib.sha256(json.dumps(nodes, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        return {'nodes': nodes, 'fingerprint': fingerprint}

    def execute(self, request):
        if request['op'] == 'list':
            return self.discover()
        if request['op'] == 'capture':
            return self.sample(request['source'])
        if request['op'] != 'dispatch':
            raise ValueError('unsupported operation')
        # Account for process startup and time spent queued at the owner.
        expires = request['expires'] / 1000
        if time.time() >= expires:
            return {'status': 'rejected'}
        capture = self.sample(request['source'])
        key = identity(request['key'])
        node = next((n for n in capture['nodes'] if n['key'] == key), None)
        action = request['action']
        if (node is None or node['fingerprint'] != request['fingerprint']
                or action not in node['actions'] or node['disabled'] or time.time() >= expires):
            return {'status': 'rejected'}
        if action == 'SetValue':
            value = text(request['value'], MAX_TEXT)
            method, interface, args = 'SetTextContents', 'EditableText', self.GLib.Variant('(s)', (value,))
        elif action == 'Activate':
            method, interface, args = 'DoAction', 'Action', self.GLib.Variant('(i)', (0,))
        else:
            return {'status': 'rejected'}
        # Once the method is attempted, a timeout must never become a retry.
        try:
            accepted = self.call(key, A + interface, method, args)[0]
            result = {'status': 'dispatched' if accepted else 'rejected'}
            try:
                result['capture'] = self.sample(request['source'])
            except Exception:
                pass  # Dispatch is known even if its subsequent observation fails.
            return result
        except Exception:
            return {'status': 'unknown'}


def main():
    resource.setrlimit(resource.RLIMIT_AS, (384 << 20, 384 << 20))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    try:
        raw = sys.stdin.buffer.read(65537)
        if len(raw) > 65536:
            raise ValueError('request ceiling')
        result = DesktopReader().execute(json.loads(raw))
        data = json.dumps(result, separators=(',', ':')).encode()
        if len(data) > 768 * 1024:
            raise ValueError('response ceiling')
    except Exception:
        data = b'{"error":"Accessibility unavailable, app changed, or observation limit reached."}'
    sys.stdout.buffer.write(data)


if __name__ == '__main__':
    main()
