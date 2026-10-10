"""Nonblocking owner for an explicitly scoped, observed semantic adapter.

The trusted launcher supplies process incarnations. Native surface association
and input remain unresolved: no title/geometry matching, actions or raw routes.
"""
import hashlib
import json
import math
import os
from pathlib import Path
import socket
import subprocess
import time
import uuid

from adapters.sdk.producer import blob, double, doubles, encode_session, ref, scalar, text
from adapters.atspi.worker import MAX_MESSAGE, MAX_NODES


class WorkerPort:
    """One in-flight crawl and one bounded response; poll never waits for AT-SPI."""
    def __init__(self, address):
        if not isinstance(address, str) or not 1 <= len(address) <= 1024:
            raise ValueError('explicit trusted accessibility bus required')
        parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        parent.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 2 * MAX_MESSAGE)
        child.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 2 * MAX_MESSAGE)
        parent.setblocking(False)
        env = os.environ.copy()
        env['AT_SPI_BUS_ADDRESS'] = address
        try:
            self.process = subprocess.Popen(['/usr/bin/python3', str(Path(__file__).with_name('worker.py')), '--fd', str(child.fileno())],
                                            pass_fds=(child.fileno(),), env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
        except Exception:
            parent.close()
            raise
        finally:
            child.close()
        self.channel = parent
        self.sequence = 0
        self.pending = None
        self.deadline = 0
        self.barrier = False

    def request(self, operation, **arguments):
        if self.pending is not None or self.process.poll() is not None:
            raise ValueError('worker busy or unavailable')
        sequence = self.sequence + 1
        if sequence >= 1 << 63:
            raise ValueError('worker request identity exhausted')
        data = json.dumps({'op': operation, 'request_id': sequence, **arguments}).encode()
        if len(data) > 4096:
            raise ValueError('worker request ceiling')
        self.channel.send(data)
        self.sequence = sequence
        self.pending = sequence
        self.deadline = time.monotonic() + 3

    def resume_fixture_barrier(self):
        if not self.barrier or self.pending is None:
            raise ValueError('no pending crawl barrier')
        self.channel.send(b'continue')
        self.barrier = False

    def poll(self):
        if self.pending is None:
            return None
        if time.monotonic() > self.deadline:
            self.abort()
            return {'type': 'unavailable', 'reason': 'worker-deadline'}
        try:
            data = self.channel.recv(MAX_MESSAGE + 1)
        except BlockingIOError:
            return None
        if not data or len(data) > MAX_MESSAGE:
            self.abort()
            return {'type': 'unavailable', 'reason': 'worker-exit-or-message-ceiling'}
        try:
            result = json.loads(data)
            if not isinstance(result, dict) or result.get('request_id') != self.pending:
                raise ValueError('stale worker result')
        except (ValueError, UnicodeError):
            self.abort()
            raise ValueError('invalid worker result') from None
        if result.get('type') == 'crawl_barrier':
            self.barrier = True
        else:
            self.pending = None
        return result

    def abort(self):
        self.pending = None
        self.barrier = False
        self.channel.close()
        if self.process.poll() is None:
            self.process.kill()

    def close(self):
        self.abort()
        self.process.wait(timeout=1)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


class ObservedProjection:
    """Atomic public identity mapping, bounded independently of observed bus IDs."""
    MAX_ISSUED = 4096
    MAX_INCARNS = 8

    def __init__(self):
        self.binding = {name: uuid.uuid4().hex for name in ('session_id', 'epoch', 'view_id')}
        self.revision = 0
        self.next_id = 2  # Session ID 1 survives source replacement.
        self.ids = {}
        self.incarnations = set()
        self.last_envelope = None
        self.last_metadata = None

    def publish(self, observation, sources):
        if observation.get('type') != 'snapshot' or observation.get('classification') != 'OBSERVED':
            raise ValueError('only a completed observed candidate can publish')
        if (len(sources) > 2 or len(observation['apps']) != len(sources)
                or set(sources) != {app['pid'] for app in observation['apps']}
                or len(set(sources.values())) != len(sources)):
            raise ValueError('candidate does not match trusted source scope')
        incarnations = self.incarnations | set(sources.values())
        if len(incarnations) > self.MAX_INCARNS or any(not isinstance(i, str) or len(i) != 32 or any(c not in '0123456789abcdef' for c in i) for i in incarnations):
            raise ValueError('source incarnation ceiling')
        revision = self.revision + 1
        if revision >= 1 << 63:
            raise ValueError('publication revision exhausted')
        ids = {}
        next_id = self.next_id

        def identifier(key):
            nonlocal next_id
            if key not in ids:
                if key in self.ids:
                    ids[key] = self.ids[key]
                else:
                    if next_id > self.MAX_ISSUED:
                        raise ValueError('public identity lifetime ceiling')
                    ids[key] = next_id
                    next_id += 1
            return ids[key]

        encoded = []
        metadata = []
        for app in observation['apps']:
            incarnation = sources[app['pid']]
            if len(app['frames']) > 4:
                raise ValueError('observed frame ceiling')
            for frame in app['frames']:
                root = tuple(frame['root'])
                records = {tuple(r['key']): r for r in frame['records']}
                if not records or len(records) != len(frame['records']) or len(records) > MAX_NODES or root not in records:
                    raise ValueError('invalid observed graph')
                reachable = set()

                def walk(key, parent, depth=0):
                    if key in reachable or depth > 16 or key not in records:
                        raise ValueError('invalid observed tree')
                    node = records[key]
                    if key[0] != app['root'][0] or node['role'] == 40 and node['name'] is not None:
                        raise ValueError('cross-application or unredacted private object')
                    if tuple(node['parent']) != parent:
                        raise ValueError('observed parent mismatch')
                    reachable.add(key)
                    for child in node['children']:
                        walk(tuple(child), key, depth + 1)
                walk(root, tuple(app['root']))
                if reachable != set(records):
                    raise ValueError('unreachable observed objects')
                surface = identifier(('surface', incarnation, *root))
                semantic_root = identifier(('semantic', incarnation, *root))
                bounds = frame['bounds']
                if len(bounds) != 4 or any(type(v) not in (int, float) or not math.isfinite(v) or abs(v) > 16384 for v in bounds) or min(bounds[2:]) <= 0:
                    raise ValueError('invalid observed bounds')
                token = 'atspi-semantics-observed:' + incarnation + ':' + hashlib.sha256(json.dumps(frame['records'], sort_keys=True).encode()).hexdigest()
                geometry_token = 'atspi-geometry-observed:' + incarnation + ':' + hashlib.sha256(json.dumps(bounds).encode()).hexdigest()
                body = scalar(1, surface) + scalar(2, revision) + scalar(3, 1)
                body += blob(5, doubles(bounds)) + blob(6, doubles(bounds[2:]))
                body += blob(7, doubles([1, 0, 0, 1, 0, 0])) + double(9, 1) + scalar(11, 1)
                body += blob(15, ref(3, semantic_root))
                # Unsupported visuals, partial sampled semantics, OBSERVED consistency.
                body += scalar(18, 3) + scalar(19, 2) + scalar(20, 2)
                body += text(21, token) + text(21, geometry_token) + text(22, 'native-surface-association-unresolved')
                encoded.append(blob(1, body))
                display_nodes = []
                for key, node in records.items():
                    children = [identifier(('semantic', incarnation, *child)) for child in node['children']]
                    role = 1 if key == root else {43: 3, 7: 4, 29: 6, 31: 7, 32: 8}.get(node['role'], 2)
                    item = {'id': identifier(('semantic', incarnation, *key)), 'version': revision,
                            'role': role, 'children': children, 'actions': []}
                    if node['name'] is not None:
                        if len(node['name'].encode()) > 256:
                            raise ValueError('observed label ceiling')
                        item['label'] = node['name']
                    semantic = scalar(1, item['id']) + scalar(2, revision) + scalar(3, role)
                    if 'label' in item:
                        semantic += text(4, item['label'])
                    semantic += b''.join(blob(8, ref(3, child)) for child in children)
                    semantic += scalar(16, 3 if node['role'] == 40 else 1) + scalar(17, 3 if node['role'] == 40 else 2)
                    encoded.append(blob(3, semantic))
                    display_nodes.append({'id':item['id'],'role':role,'children':children,
                                          'name':item.get('label'),'availability':'redacted' if node['role']==40 else 'partial',
                                          'actions':[]})
                metadata.append({'surface': surface, 'semantic_root': semantic_root,
                                 'classification': 'OBSERVED', 'source_incarnation': incarnation,
                                 'native_surface_association': 'unresolved', 'input_modes': [],
                                 'record_count': len(records), 'bounds': bounds, 'nodes':display_nodes})
        if len(encoded) > 2 * MAX_NODES + 8:
            raise ValueError('projection ceiling')
        encoded.append(encode_session({'id': 1, 'version': revision, 'canvas': [0, 0, 800, 600], 'focus_generation': 1}))
        features = ['content.no-raster.v1', 'mirror2d.core.v1', 'semantics.core.v1']
        payload = scalar(1, revision) + b''.join(blob(2, record) for record in encoded)
        payload += b''.join(text(4, feature) for feature in features)
        envelope = scalar(1, 1) + b''.join(blob(tag, bytes.fromhex(self.binding[name])) for tag, name in ((2, 'session_id'), (3, 'epoch'), (4, 'view_id')))
        envelope += scalar(5, 1) + blob(6, payload) + b''.join(text(7, feature) for feature in features)
        if len(envelope) > MAX_MESSAGE:
            raise ValueError('public projection byte ceiling')
        self.ids, self.next_id, self.incarnations = ids, next_id, incarnations
        self.revision, self.last_envelope, self.last_metadata = revision, envelope, metadata
        return envelope, metadata
