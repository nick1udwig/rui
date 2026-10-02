#!/usr/bin/env python3
"""Independent, bounded Python semantic client for the controlled RUI profile.

Uses only Python's standard library: no generated protobuf, Rust reducer, GTK,
scene renderer, coordinates, or synthetic Presented acknowledgements. The local
operator supplies the approved credential and exact trusted server certificate.
"""
from __future__ import annotations

import base64
import copy
import hashlib
import math
import os
import socket
import ssl
import struct


class ProtocolError(ValueError):
    pass


def uint(value):
    if not 0 <= value < 1 << 64:
        raise ProtocolError('integer outside uint64')
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def scalar(tag, value):
    return uint(tag << 3) + uint(value)


def blob(tag, value):
    if isinstance(value, str):
        value = value.encode('utf-8')
    return uint((tag << 3) | 2) + uint(len(value)) + value


def read_uint(data, offset):
    result = 0
    for shift in range(0, 70, 7):
        if offset == len(data):
            raise ProtocolError('truncated integer')
        byte = data[offset]
        offset += 1
        if shift == 63 and byte > 1:
            raise ProtocolError('integer overflow')
        result |= (byte & 127) << shift
        if byte < 128:
            return result, offset
    raise ProtocolError('integer overflow')


def decode(data, allowed, maximum=1 << 20):
    """Flat bounded parser; each explicit caller owns nested vocabulary."""
    if len(data) > maximum:
        raise ProtocolError('message ceiling')
    result, offset, count = {}, 0, 0
    while offset < len(data):
        count += 1
        if count > 20_000:
            raise ProtocolError('field ceiling')
        key, offset = read_uint(data, offset)
        tag, wire = key >> 3, key & 7
        if not 0 < tag < 1 << 29 or (tag < 128 and tag not in allowed):
            raise ProtocolError('unknown required vocabulary')
        if wire == 0:
            value, offset = read_uint(data, offset)
        elif wire in (1, 2, 5):
            if wire == 2:
                size, offset = read_uint(data, offset)
            else:
                size = 8 if wire == 1 else 4
            end = offset + size
            if end > len(data):
                raise ProtocolError('truncated field')
            value, offset = data[offset:end], end
        else:
            raise ProtocolError('unsupported wire type')
        if tag in allowed:
            result.setdefault(tag, []).append((wire, value))
    return result


def repeated(message, tag, wire=2):
    entries = message.get(tag, [])
    if any(actual != wire for actual, _ in entries):
        raise ProtocolError('wrong wire type')
    return [value for _, value in entries]


def value(message, tag, wire=0, default=0):
    entries = repeated(message, tag, wire)
    # This client deliberately rejects duplicate singular fields rather than
    # relying on merge order for authority-bearing state.
    if len(entries) > 1:
        raise ProtocolError('duplicate singular field')
    return entries[0] if entries else default


def binary(message, tag, default=b''):
    return value(message, tag, 2, default)


def text(message, tag, default=None):
    result = binary(message, tag, None)
    return default if result is None else result.decode('utf-8', errors='strict')


def identifier(message, tag):
    result = binary(message, tag)
    if len(result) != 16:
        raise ProtocolError('invalid binding identity')
    return result


def reference(data):
    item = decode(data, {1, 2})
    kind, identity = value(item, 1), value(item, 2)
    if kind not in range(1, 7) or identity == 0:
        raise ProtocolError('invalid object reference')
    return kind, identity


def geometry(raw, count, dimensions=()):
    """Validate required topology metadata; never expose it to action selection."""
    fields = decode(raw, set(range(1, count + 1)))
    values = [struct.unpack('<d', value(fields, tag, 1, bytes(8)))[0] for tag in range(1, count + 1)]
    if any(not math.isfinite(number) or abs(number) > 1_000_000_000 for number in values):
        raise ProtocolError('invalid topology geometry')
    if any(values[index] < 0 for index in dimensions):
        raise ProtocolError('negative topology dimension')
    if count == 4 and any(abs(values[index] + values[index + 2]) > 1_000_000_000 for index in (0, 1)):
        raise ProtocolError('topology rectangle overflow')


def known_record(raw):
    """Remove optional metadata recursively for unchanged-record comparison."""
    ref_schema = ({1, 2}, {}, set())
    rect = (set(range(1, 5)), {}, set())
    shapes = {
        1: (set(range(1, 23)), {4: ref_schema, 5: rect, 6: ref_schema, 7: (set(range(1, 7)), {}, set()),
                              8: rect, 13: ref_schema, 15: ref_schema, 16: rect}, set()),
        3: (set(range(1, 18)), {6: (set(range(1, 5)), {}, {1, 2, 3, 4}), 7: ref_schema, 8: ref_schema,
                              9: ({1, 2}, {2: ref_schema}, set()), 10: ref_schema, 11: ref_schema,
                              12: (set(range(1, 6)), {2: ref_schema, 5: ref_schema}, {3}), 14: ref_schema}, {4, 5, 15}),
        4: (set(range(1, 9)), {6: ref_schema}, {5}),
        6: (set(range(1, 8)), {3: rect, 5: ref_schema, 6: ref_schema}, set()),
    }

    def canonical(data, schema):
        allowed, children, presence = schema
        fields = decode(data, allowed)
        result = {}
        for tag, entries in fields.items():
            normalized = []
            for wire, entry in entries:
                if tag in children and wire == 2:
                    entry = canonical(entry, children[tag])
                elif wire == 1:
                    entry = struct.unpack('<d', entry)[0]
                if tag not in presence and wire in (0, 1) and entry == 0:
                    continue
                normalized.append((wire, entry))
            if normalized:
                result[tag] = normalized
        return result

    return canonical(raw, (set(shapes), shapes, set()))


class PinnedWebSocket:
    """Uncompressed bounded RFC6455 client over normally validated pinned TLS."""

    def __init__(self, address, hostname, certificate_der, ceiling=65536):
        if not 512 <= ceiling <= 1 << 20 or any(c in hostname for c in '\r\n'):
            raise ProtocolError('invalid connection configuration')
        self.ceiling = ceiling
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_verify_locations(cadata=ssl.DER_cert_to_PEM_cert(certificate_der))
        connection = socket.create_connection(address, timeout=10)
        try:
            self.socket = context.wrap_socket(connection, server_hostname=hostname)
            if self.socket.getpeercert(binary_form=True) != certificate_der:
                raise ProtocolError('server certificate pin mismatch')
            key = base64.b64encode(os.urandom(16)).decode()
            request = (f'GET /rui HTTP/1.1\r\nHost: {hostname}:{address[1]}\r\n'
                       f'Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n'
                       'Sec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: rui.v1\r\n'
                       f'x-rui-envelope-limit: {ceiling}\r\n\r\n')
            self.socket.sendall(request.encode('ascii'))
            response = bytearray()
            while not response.endswith(b'\r\n\r\n'):
                if len(response) >= 8192:
                    raise ProtocolError('HTTP header ceiling')
                response.extend(self._read(1))
            lines = response.decode('ascii').split('\r\n')
            headers = {}
            for line in lines[1:-2]:
                name, separator, content = line.partition(':')
                if not separator or name.lower() in headers:
                    raise ProtocolError('invalid duplicate HTTP header')
                headers[name.lower()] = content.strip()
            accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
            if (lines[0] != 'HTTP/1.1 101 Switching Protocols'
                    or headers.get('sec-websocket-accept') != accept
                    or headers.get('sec-websocket-protocol') != 'rui.v1'
                    or headers.get('upgrade', '').lower() != 'websocket'
                    or 'upgrade' not in headers.get('connection', '').lower().split(', ')
                    or 'sec-websocket-extensions' in headers):
                raise ProtocolError('invalid WebSocket upgrade')
            selected = int(headers.get('x-rui-envelope-limit', '0'))
            if not 512 <= selected <= ceiling:
                raise ProtocolError('invalid negotiated frame ceiling')
            self.ceiling = selected
        except BaseException:
            getattr(self, 'socket', connection).close()
            raise

    def _read(self, count):
        result = bytearray()
        while len(result) < count:
            part = self.socket.recv(count - len(result))
            if not part:
                raise EOFError('connection closed')
            result.extend(part)
        return bytes(result)

    def send(self, data, opcode=2):
        if len(data) > self.ceiling or (opcode >= 8 and len(data) > 125):
            raise ProtocolError('outbound frame ceiling')
        size = len(data)
        prefix = bytes([128 | opcode])
        if size < 126:
            prefix += bytes([128 | size])
        elif size <= 65535:
            prefix += b'\xfe' + struct.pack('!H', size)
        else:
            prefix += b'\xff' + struct.pack('!Q', size)
        mask = os.urandom(4)
        masked = bytes(byte ^ mask[index % 4] for index, byte in enumerate(data))
        self.socket.sendall(prefix + mask + masked)

    def receive(self):
        for _ in range(64):
            first, second = self._read(2)
            opcode, size = first & 15, second & 127
            if first & 112 or not first & 128 or second & 128:
                raise ProtocolError('unsupported compressed, fragmented or masked server frame')
            if size == 126:
                size = struct.unpack('!H', self._read(2))[0]
                if size < 126:
                    raise ProtocolError('noncanonical frame length')
            elif size == 127:
                size = struct.unpack('!Q', self._read(8))[0]
                if size <= 65535:
                    raise ProtocolError('noncanonical frame length')
            if size > self.ceiling or (opcode >= 8 and size > 125):
                raise ProtocolError('inbound frame ceiling')
            data = self._read(size)
            if opcode == 2:
                return data
            if opcode == 9:
                self.send(data, 10)
            elif opcode == 8:
                raise EOFError('server closed WebSocket')
            elif opcode != 10:
                raise ProtocolError('expected binary frame')
        raise ProtocolError('control frame flood')

    def close(self):
        self.socket.close()


class SemanticReplica:
    """Atomic semantic/text/session projection with stable IDs and typed clocks."""

    def __init__(self, record_limit=4096, depth_limit=128):
        if not 0 < record_limit <= 4096 or not 0 < depth_limit <= 128:
            raise ProtocolError('invalid semantic replica limits')
        self.record_limit = record_limit
        self.depth_limit = depth_limit
        self.revision = 0
        self.records = {}
        self.retired = set()
        self.history = {}

    @staticmethod
    def record(raw):
        wrapper = decode(raw, set(range(1, 7)))
        kinds = [kind for kind in range(1, 7) if kind in wrapper]
        if len(kinds) != 1 or kinds[0] not in (1, 3, 4, 6):
            raise ProtocolError('semantic-only projection required')
        kind = kinds[0]
        record = decode(binary(wrapper, kind), set(range(1, {1: 23, 3: 18, 4: 9, 6: 8}[kind])))
        identity, version = value(record, 1), value(record, 2)
        if not identity or not version:
            raise ProtocolError('zero record identity or version')
        result = {'kind': kind, 'id': identity, 'version': version, 'refs': [], 'children': [], '_wire': raw}
        if kind == 3:
            role = value(record, 3)
            if role not in range(1, 10) or value(record, 16) not in range(1, 4) or value(record, 17) not in range(1, 4):
                raise ProtocolError('invalid semantic enum')
            if value(record, 16) == 3 and any(tag in record for tag in (4, 5, 6)):
                raise ProtocolError('redacted node contains private content')
            result.update(role=role, label=text(record, 4), actions={}, states={})
            for raw_action in repeated(record, 10):
                action = decode(raw_action, {1, 2})
                name, schema = text(action, 1), value(action, 2)
                if not name or not schema or name in result['actions']:
                    raise ProtocolError('invalid action')
                result['actions'][name] = schema
            for raw_state in repeated(record, 7):
                state = decode(raw_state, {1, 2})
                key, status = value(state, 1), value(state, 2)
                if key not in range(1, 6) or status not in range(1, 4) or key in result['states']:
                    raise ProtocolError('invalid state')
                result['states'][key] = status
            result['children'] = [reference(raw) for raw in repeated(record, 8)]
            result['refs'].extend(result['children'])
            for tag in (11, 14):
                if tag in record:
                    ref = reference(binary(record, tag))
                    if ref[0] != (4 if tag == 11 else 3):
                        raise ProtocolError('wrong semantic reference kind')
                    result['refs'].append(ref)
                    result['text_model' if tag == 11 else 'semantic_group'] = ref[1]
            if 13 in record:
                raise ProtocolError('visual binding in semantic-only projection')
            if 6 in record:
                typed = decode(binary(record, 6), {1, 2, 3, 4})
                if len(typed) != 1:
                    raise ProtocolError('invalid typed value')
                tag = next(iter(typed))
                if tag == 1:
                    typed_value = text(typed, tag)
                elif tag == 2:
                    typed_value = value(typed, tag)
                    if typed_value not in (0, 1):
                        raise ProtocolError('invalid boolean value')
                    typed_value = bool(typed_value)
                elif tag == 3:
                    typed_value = struct.unpack('<d', value(typed, tag, 1))[0]
                    if not math.isfinite(typed_value):
                        raise ProtocolError('nonfinite numeric value')
                else:
                    encoded = value(typed, tag)
                    typed_value = (encoded >> 1) ^ -(encoded & 1)
                result['value'] = typed_value
                result['value_kind'] = tag
            for relation_raw in repeated(record, 9):
                relation = decode(relation_raw, {1, 2})
                if value(relation, 1) not in range(1, 6):
                    raise ProtocolError('unknown relation')
                refs = [reference(raw) for raw in repeated(relation, 2)]
                if any(ref[0] != 3 for ref in refs):
                    raise ProtocolError('relation target must be semantic')
                result['refs'].extend(refs)
            if 12 in record:
                collection = decode(binary(record, 12), {1, 2, 3, 4, 5})
                items = [reference(raw) for raw in repeated(collection, 2)]
                selected = [reference(raw) for raw in repeated(collection, 5)]
                if (not value(collection, 1) or value(collection, 4) not in (1, 2)
                        or len(set(items)) != len(items) or len(set(selected)) != len(selected)
                        or any(ref[0] != 3 for ref in items + selected)
                        or not set(selected).issubset(items) or not set(items).issubset(result['children'])):
                    raise ProtocolError('invalid collection')
                total = value(collection, 3, default=None)
                if total is not None and (total < len(items) or (value(collection, 4) == 1 and total != len(items))):
                    raise ProtocolError('invalid collection count')
                result['collection'] = {'version': value(collection, 1), 'items': [ref[1] for ref in items], 'selected': [ref[1] for ref in selected], 'total': total, 'materialization': value(collection, 4)}
                result['refs'].extend(items + selected)
        elif kind == 4:
            status = value(record, 4)
            content = text(record, 5)
            if status not in (1, 2, 3) or not value(record, 3) or (status == 1) != (content is not None):
                raise ProtocolError('invalid text availability')
            if status != 1 and value(record, 7):
                raise ProtocolError('unavailable text cannot be editable')
            result.update(text_version=value(record, 3), status=status, content=content, editable=bool(value(record, 7)), selectable=bool(value(record, 8)), selection=None)
            if 6 in record:
                selection = decode(binary(record, 6), {1, 2})
                anchor, focus = value(selection, 1), value(selection, 2)
                if content is None or not result['selectable'] or max(anchor, focus) > len(content):
                    raise ProtocolError('invalid Unicode scalar selection')
                result['selection'] = (anchor, focus)
        elif kind == 6:
            if 4 in record:
                raise ProtocolError('visual session state in semantic-only projection')
            if 3 not in record:
                raise ProtocolError('missing session canvas metadata')
            geometry(binary(record, 3), 4, (2, 3))
            result['focus_generation'] = value(record, 7)
            if 5 in record:
                ref = reference(binary(record, 5))
                if ref[0] != 3:
                    raise ProtocolError('invalid focus kind')
                result['refs'].append(ref)
                result['focus'] = ref[1]
            if 6 in record:
                ref = reference(binary(record, 6))
                if ref[0] != 1:
                    raise ProtocolError('invalid focus surface kind')
                result['refs'].append(ref)
                result['focus_surface'] = ref[1]
        else:
            if 14 in record or value(record, 18) != 3:
                raise ProtocolError('visual scene in semantic-only surface')
            if (value(record, 3) not in (1, 2, 3, 4) or value(record, 19) not in (1, 2, 3)
                    or value(record, 20) not in (1, 2) or not repeated(record, 22)
                    or (value(record, 20) == 1 and not repeated(record, 21))
                    or (value(record, 3) in (2, 3) and 4 not in record)):
                raise ProtocolError('invalid surface role, provenance or coverage')
            tokens = repeated(record, 21)
            if any(not token for token in tokens) or (value(record, 20) == 2 and len(set(tokens)) < 2):
                raise ProtocolError('invalid observed/coherent capture tokens')
            for tag, count, dimensions in ((5, 4, (2, 3)), (6, 2, (0, 1)), (7, 6, ())):
                if tag not in record:
                    raise ProtocolError('missing surface topology metadata')
                geometry(binary(record, tag), count, dimensions)
            for raw_rectangle in repeated(record, 8) + repeated(record, 16):
                geometry(raw_rectangle, 4, (2, 3))
            opacity = struct.unpack('<d', value(record, 9, 1, bytes(8)))[0]
            if not 0 <= opacity <= 1:
                raise ProtocolError('invalid topology opacity')
            result.update(mapped=bool(value(record, 11)))
            for tag in (4, 13, 15):
                if tag in record:
                    ref = reference(binary(record, tag))
                    if ref[0] != (3 if tag == 15 else 1):
                        raise ProtocolError('invalid surface reference kind')
                    result['refs'].append(ref)
                    if tag == 4:
                        result['parent'] = ref[1]
                    elif tag == 15:
                        result['semantic_root'] = ref[1]
                    if tag in (4, 13) and ref[1] == identity:
                        raise ProtocolError('surface references itself')
        return result

    @staticmethod
    def validate_clocks(previous, record, transaction):
        if previous is None:
            if transaction and record['version'] != 1:
                raise ProtocolError('new record version must be one')
            return
        if (previous['kind'] != record['kind'] or record['version'] < previous['version']
                or (transaction and record['version'] != previous['version'] + 1)
                or (record['version'] == previous['version'] and known_record(record['_wire']) != known_record(previous['_wire']))):
            raise ProtocolError('record version or kind mismatch')
        if record['kind'] == 4:
            before, after = previous['text_version'], record['text_version']
            changed = previous['content'] != record['content'] or previous['status'] != record['status']
        elif record['kind'] == 3 and 'collection' not in previous and 'collection' in record:
            if transaction and record['collection']['version'] != 1:
                raise ProtocolError('new collection clock must be one')
            return
        elif record['kind'] == 3 and 'collection' in previous and 'collection' in record:
            before, after = previous['collection']['version'], record['collection']['version']
            changed = any(previous['collection'][key] != record['collection'][key]
                          for key in ('items', 'selected', 'total', 'materialization'))
        elif record['kind'] == 6:
            before, after = previous['focus_generation'], record['focus_generation']
            changed = any(previous.get(key) != record.get(key) for key in ('focus', 'focus_surface'))
        else:
            return
        if (transaction and after != before + int(changed)) or after < before or (changed and after <= before):
            raise ProtocolError('typed source clock did not advance')

    def apply(self, kind, raw):
        digest = hashlib.sha256(raw).digest()
        message = decode(raw, {1, 2, 3, 4} if kind == 1 else {1, 2, 3, 4, 5})
        announced = value(message, 1 if kind == 1 else 2)
        if announced <= self.revision:
            if self.history.get(announced) == (kind, digest):
                return
            raise ProtocolError('conflicting or expired duplicate publication')
        candidate = copy.deepcopy(self.records)
        retired = self.retired.copy()
        touched = set()
        if kind == 1:
            revision = value(message, 1)
            if revision <= self.revision:
                raise ProtocolError('snapshot revision did not advance')
            records = [self.record(record) for record in repeated(message, 2)]
            candidate = {}
            for record in records:
                identity = record['id']
                if identity in candidate or identity in retired:
                    raise ProtocolError('duplicate or retired identity')
                self.validate_clocks(self.records.get(identity), record, False)
                candidate[identity] = record
            retired.update(self.records.keys() - candidate.keys())
        elif kind == 2:
            revision = value(message, 2)
            if value(message, 1) != self.revision or revision != self.revision + 1:
                raise ProtocolError('transaction base mismatch')
            for raw_mutation in repeated(message, 3):
                mutation = decode(raw_mutation, {1, 2})
                if len(mutation) != 1:
                    raise ProtocolError('invalid mutation')
                if 1 in mutation:
                    record = self.record(binary(mutation, 1))
                    identity = record['id']
                    previous = candidate.get(identity)
                    if identity in retired:
                        raise ProtocolError('retired identity')
                    self.validate_clocks(previous, record, True)
                    candidate[identity] = record
                else:
                    ref = reference(binary(mutation, 2))
                    identity = ref[1]
                    if identity not in candidate or candidate[identity]['kind'] != ref[0]:
                        raise ProtocolError('invalid deletion')
                    del candidate[identity]
                    retired.add(identity)
                if identity in touched:
                    raise ProtocolError('duplicate mutation')
                touched.add(identity)
        else:
            raise ProtocolError('unknown publication kind')
        if repeated(message, 3 if kind == 1 else 4):
            raise ProtocolError('semantic projection requires no visual resources')
        features = [item.decode('utf-8') for item in repeated(message, 4 if kind == 1 else 5)]
        if any(feature not in ('content.no-raster.v1', 'semantics.core.v1', 'mirror2d.core.v1', 'semantics.only.v1') for feature in features):
            raise ProtocolError('unsupported required feature')
        if len(candidate) > self.record_limit or len(retired) > 20_000:
            raise ProtocolError('record or retirement ceiling')
        owner = {}
        roots = {}
        surfaces = 0
        sessions = 0
        for record in candidate.values():
            if record['kind'] == 1:
                surfaces += 1
                root = record.get('semantic_root')
                if root is not None:
                    if root in roots:
                        raise ProtocolError('multiple surface root ownership')
                    roots[root] = record['id']
            sessions += record['kind'] == 6
            for ref in record['refs']:
                if ref[1] not in candidate or candidate[ref[1]]['kind'] != ref[0]:
                    raise ProtocolError('dangling typed reference')
            for ref in record['children']:
                if ref[0] != 3 or ref[1] in owner:
                    raise ProtocolError('multiple or invalid semantic ownership')
                owner[ref[1]] = record['id']
        if sessions > 1 or any(identity in owner for identity in roots):
            raise ProtocolError('duplicate session or owned surface root')
        semantic_surfaces = {}
        for identity in candidate:
            original = identity
            seen = set()
            while identity in owner:
                if identity in seen or len(seen) >= self.depth_limit - 1:
                    raise ProtocolError('ownership cycle or depth')
                seen.add(identity)
                identity = owner[identity]
            if candidate[original]['kind'] == 3:
                if surfaces and identity not in roots:
                    raise ProtocolError('semantic tree has no owning surface')
                semantic_surfaces[original] = roots.get(identity)
        for record in candidate.values():
            if record['kind'] == 6 and surfaces and 'focus' in record:
                if semantic_surfaces[record['focus']] != record.get('focus_surface'):
                    raise ProtocolError('focus belongs to another surface')
            seen = set()
            while 'parent' in record:
                if record['id'] in seen or len(seen) >= self.depth_limit - 1:
                    raise ProtocolError('surface topology cycle or depth')
                seen.add(record['id'])
                record = candidate[record['parent']]
        self.records, self.retired, self.revision = candidate, retired, revision
        self.history[revision] = (kind, digest)
        while len(self.history) > 256:
            del self.history[next(iter(self.history))]

    def discover(self, *, role=None, action=None, label=None):
        return [record for record in self.records.values() if record['kind'] == 3
                and (role is None or record['role'] == role)
                and (action is None or action in record['actions'])
                and (label is None or record['label'] == label)]


class SemanticClient:
    PROFILES = ('content.no-raster.v1', 'mirror2d.core.v1', 'semantics.core.v1', 'semantics.only.v1')
    # These are explicit independent-client ceilings, not a claim about native
    # Python allocation accounting. Every retained table also has a count bound.
    LIMITS = (65536, 16384, 1 << 20, 1 << 20, 1 << 20, 32 << 20, 2 << 20, 4096, 128)

    def __init__(self, address, hostname, certificate_der, credential, instance_id):
        if len(credential) != 32 or len(instance_id) != 16:
            raise ProtocolError('invalid local approved identity')
        self.wire = PinnedWebSocket(address, hostname, certificate_der, self.LIMITS[0])
        self.instance_id = instance_id
        self.binding = None
        self.replica = SemanticReplica()
        self.pending = None
        self.lease = None
        self.sequence = 0
        self.last_command = None
        self.last_result = None
        try:
            limits = b''.join(scalar(tag, self.wire.ceiling if tag == 1 else count) for tag, count in enumerate(self.LIMITS, 1))
            hello = blob(1, credential) + blob(2, instance_id) + scalar(3, 1) + blob(6, limits)
            hello += b''.join(blob(tag, profile) for tag in (4, 5) for profile in self.PROFILES)
            self.send(1, hello)
            kind, payload = self.packet()
            if kind != 2:
                raise ProtocolError('server did not negotiate')
            server = decode(payload, {1, 2, 3, 4, 5})
            selected = {item.decode('utf-8') for item in repeated(server, 2)}
            if value(server, 1) != 1 or selected != set(self.PROFILES):
                raise ProtocolError('semantic-only profile not negotiated')
            negotiated = decode(binary(server, 3), set(range(1, 10)))
            self.limits = tuple(value(negotiated, tag) for tag in range(1, 10))
            if (any(not 0 < actual <= maximum for actual, maximum in zip(self.limits, self.LIMITS))
                    or self.limits[0] != self.wire.ceiling):
                raise ProtocolError('invalid negotiated ceilings')
            self.replica = SemanticReplica(self.limits[7], self.limits[8])
            self.send(3, blob(1, 'fixture') + blob(2, 'semantic-only'))
        except BaseException:
            self.wire.close()
            raise

    def send(self, kind, payload):
        raw = scalar(4, 1) + scalar(5, kind) + blob(6, payload)
        if self.binding:
            raw += b''.join(blob(tag, identity) for tag, identity in enumerate(self.binding, 1))
        self.wire.send(raw)

    def packet(self):
        packet = decode(self.wire.receive(), set(range(1, 8)), self.wire.ceiling)
        if value(packet, 4) != 1 or repeated(packet, 7):
            raise ProtocolError('unsupported packet version or features')
        binding = tuple(identifier(packet, tag) for tag in (1, 2, 3))
        if self.binding is None:
            self.binding = binding
        elif self.binding != binding:
            raise ProtocolError('packet changed authenticated view binding')
        return value(packet, 5), binary(packet, 6)

    def receive(self):
        for _ in range(4096):
            kind, payload = self.packet()
            if kind == 6:
                ping = decode(payload, {1})
                self.send(7, scalar(1, value(ping, 1)))
            elif kind == 4:
                opened = decode(payload, {1, 2, 3, 4, 5})
                if identifier(opened, 1) != self.binding[2]:
                    raise ProtocolError('opened wrong view')
                return 'opened', None
            elif kind in (10, 11):
                begin = decode(payload, set(range(1, 8)))
                total, count = value(begin, 4), value(begin, 5)
                if (self.pending is not None or not 0 < total <= self.limits[3 if kind == 10 else 2]
                        or not 0 < count <= 4096 or value(begin, 6) > self.limits[7]
                        or len(binary(begin, 7)) != 32):
                    raise ProtocolError('invalid bounded publication begin')
                self.pending = {'kind': kind - 9, 'id': identifier(begin, 1), 'base': value(begin, 2),
                                'revision': value(begin, 3), 'total': total, 'count': count,
                                'items': value(begin, 6), 'digest': binary(begin, 7), 'chunks': {}, 'received': 0}
            elif kind == 12:
                chunk = decode(payload, {1, 2, 3})
                pending = self.pending
                index, data = value(chunk, 2), binary(chunk, 3)
                if (pending is None or identifier(chunk, 1) != pending['id'] or index >= pending['count']
                        or not 0 < len(data) <= self.limits[1]):
                    raise ProtocolError('invalid publication chunk')
                if index in pending['chunks']:
                    if data != pending['chunks'][index]:
                        raise ProtocolError('conflicting duplicate chunk')
                else:
                    if pending['received'] + len(data) > pending['total']:
                        raise ProtocolError('publication length ceiling')
                    pending['chunks'][index] = data
                    pending['received'] += len(data)
            elif kind == 13:
                commit = decode(payload, {1, 2, 3, 4, 5})
                pending = self.pending
                if (pending is None or identifier(commit, 1) != pending['id']
                        or value(commit, 2) != pending['total'] or value(commit, 3) != pending['count']
                        or value(commit, 4) != pending['items'] or binary(commit, 5) != pending['digest']
                        or pending['received'] != pending['total'] or len(pending['chunks']) != pending['count']):
                    raise ProtocolError('publication commit mismatch')
                raw = b''.join(pending['chunks'][index] for index in range(pending['count']))
                if hashlib.sha256(raw).digest() != pending['digest']:
                    raise ProtocolError('publication digest mismatch')
                # Transport chunks contain exact Snapshot/Transaction bytes.
                # Session/epoch/view binding is on every authenticated Packet.
                body = raw
                inner = decode(body, {1, 2, 3, 4} if pending['kind'] == 1 else {1, 2, 3, 4, 5})
                actual_revision = value(inner, 1 if pending['kind'] == 1 else 2)
                if (actual_revision != pending['revision']
                        or len(repeated(inner, 2 if pending['kind'] == 1 else 3)) != pending['items']
                        or (pending['kind'] == 2 and value(inner, 1) != pending['base'])):
                    raise ProtocolError('publication metadata mismatch')
                self.replica.apply(pending['kind'], body)
                self.pending = None
                self.send(30, scalar(1, self.replica.revision))
                self.send(35, scalar(1, self.replica.revision))
                return 'applied', self.replica.revision
            elif kind == 14:
                abort = decode(payload, {1})
                if self.pending is None or identifier(abort, 1) != self.pending['id']:
                    raise ProtocolError('unknown publication abort')
                self.pending = None
            elif kind == 41:
                granted = decode(payload, {1, 2, 3, 4, 5})
                self.lease = (identifier(granted, 1), value(granted, 2))
                if not self.lease[1]:
                    raise ProtocolError('zero lease generation')
                return 'control', self.lease
            elif kind == 44:
                self.lease = None
                return 'revoked', None
            elif kind == 54:
                result = decode(payload, {1, 2, 3, 4, 5})
                status = text(result, 2)
                if (value(result, 1) != self.sequence or status not in
                        ('Accepted', 'Dispatched', 'Applied', 'Rejected', 'Failed', 'OutcomeUnknown', 'OutcomeExpired')):
                    raise ProtocolError('invalid command result')
                evidence = text(result, 4)
                if status == 'Applied' and not evidence:
                    raise ProtocolError('Applied result lacks source evidence')
                self.last_result = {'sequence': self.sequence, 'status': status, 'rejection': text(result, 3),
                                    'source_commit': evidence, 'revision': value(result, 5, default=None)}
                return 'result', self.last_result
            elif kind == 8:
                raise ProtocolError('host rejected protocol operation: ' + str(text(decode(payload, {1}), 1)))
            else:
                raise ProtocolError(f'unsupported semantic-only packet kind {kind}')
        raise ProtocolError('bounded receive event ceiling')

    def acquire(self):
        if not self.replica.revision:
            raise ProtocolError('semantic state not ready')
        self.send(40, blob(1, os.urandom(16)))

    def command(self, target_id, action, *, replacement=None, selection=None, checked=None):
        if not self.lease:
            raise ProtocolError('control not granted')
        target = self.replica.records.get(target_id)
        if target is None or target['kind'] != 3 or action not in target['actions']:
            raise ProtocolError('target does not advertise requested action')
        preconditions = scalar(1, target['version'])
        text_model = None
        if 'text_model' in target:
            text_model = self.replica.records[target['text_model']]
            preconditions += scalar(2, text_model['text_version'])
            focused = [record for record in self.replica.records.values()
                       if record['kind'] == 6 and record.get('focus') == target_id]
            if len(focused) == 1:
                preconditions += scalar(4, focused[0]['focus_generation'])
        if 'collection' in target:
            preconditions += scalar(3, target['collection']['version'])
        arguments = blob(12, b'')
        if replacement is not None:
            start, end, inserted = replacement
            if action != 'ReplaceText' or text_model is None or text_model['content'] is None or not 0 <= start <= end <= len(text_model['content']):
                raise ProtocolError('invalid scalar replacement')
            arguments = blob(14, scalar(1, start) + scalar(2, end) + blob(3, inserted))
        elif selection is not None:
            anchor, focus = selection
            if action != 'SetSelection' or text_model is None or text_model['content'] is None or not 0 <= min(anchor, focus) <= max(anchor, focus) <= len(text_model['content']):
                raise ProtocolError('invalid scalar selection')
            arguments = blob(15, scalar(1, anchor) + scalar(2, focus))
        elif checked is not None:
            if action != 'SetValue' or not isinstance(checked, bool):
                raise ProtocolError('invalid checked value')
            arguments = blob(13, scalar(1, int(checked)))
        self.sequence += 1
        session, epoch, view = self.binding
        raw = (blob(1, epoch) + blob(2, view) + blob(3, self.instance_id) + scalar(4, self.sequence)
               + blob(5, self.lease[0]) + scalar(6, self.lease[1]) + scalar(7, target_id)
               + blob(8, action) + scalar(9, target['actions'][action]) + scalar(10, self.replica.revision)
               + blob(11, preconditions) + arguments)
        self.last_command = raw
        self.send(50, raw)

    def retry_last(self):
        if self.last_command is None:
            raise ProtocolError('no command to retry')
        self.send(50, self.last_command)

    def close(self):
        self.wire.close()


def await_event(client, desired):
    for _ in range(256):
        event, result = client.receive()
        if event == desired:
            return result
        if event == 'revoked':
            raise ProtocolError('control revoked during semantic operation')
    raise ProtocolError('event wait ceiling')


def unique_target(client, *, role, label, action):
    matches = client.replica.discover(role=role, label=label, action=action)
    if len(matches) != 1:
        raise ProtocolError('semantic selector is absent or ambiguous')
    return matches[0]


def applied_result(client, *, verify_retry=False):
    result = await_event(client, 'result')
    if result['status'] != 'Applied':
        raise ProtocolError('source did not confirm semantic operation')
    if verify_retry:
        client.retry_last()
        if await_event(client, 'result') != result:
            raise ProtocolError('exact retry changed source result')
    revision = result['revision']
    if revision is not None:
        for _ in range(256):
            if client.replica.revision >= revision:
                break
            client.receive()
        else:
            raise ProtocolError('source result revision did not converge')
    return result


def main():
    import argparse
    import json
    from pathlib import Path

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--address', default='127.0.0.1')
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--hostname', default='localhost')
    parser.add_argument('--certificate', type=Path, required=True)
    parser.add_argument('--credential', type=Path, required=True)
    parser.add_argument('--instance', required=True, help='approved 16-byte client ID in hex')
    parser.add_argument('--activate-label')
    parser.add_argument('--replace-label')
    parser.add_argument('--replacement', default='Aé🦀Z')
    parser.add_argument('--select-item-label')
    args = parser.parse_args()
    with args.credential.open('rb') as stream:
        credential = stream.read(33)
    with args.certificate.open('rb') as stream:
        certificate = stream.read(65537)
    if len(certificate) > 65536:
        raise ProtocolError('certificate file ceiling')
    client = SemanticClient((args.address, args.port), args.hostname, certificate, credential, bytes.fromhex(args.instance))
    del credential
    results = []
    try:
        await_event(client, 'applied')
        client.acquire()
        await_event(client, 'control')
        if args.activate_label:
            target = unique_target(client, role=3, label=args.activate_label, action='Activate')
            client.command(target['id'], 'Activate')
            results.append(applied_result(client, verify_retry=True))
        if args.replace_label:
            target = unique_target(client, role=5, label=args.replace_label, action='ReplaceText')
            model = client.replica.records[target['text_model']]
            client.command(target['id'], 'ReplaceText', replacement=(0, len(model['content']), args.replacement))
            results.append(applied_result(client))
            if client.replica.records[target['text_model']]['content'] != args.replacement:
                raise ProtocolError('Unicode source edit did not converge')
        if args.select_item_label:
            target = unique_target(client, role=8, label=args.select_item_label, action='Activate')
            owners = [record for record in client.replica.discover(role=7)
                      if target['id'] in record.get('collection', {}).get('items', [])]
            if len(owners) != 1:
                raise ProtocolError('item has no unambiguous materialized collection')
            client.command(target['id'], 'Activate')
            results.append(applied_result(client))
            if target['id'] not in client.replica.records[owners[0]['id']]['collection']['selected']:
                raise ProtocolError('stable item selection did not converge')
        print(json.dumps({'revision': client.replica.revision, 'records': len(client.replica.records),
                          'command_results': results, 'presented_acknowledgements': 0}, sort_keys=True))
    finally:
        client.close()


if __name__ == '__main__':
    main()
