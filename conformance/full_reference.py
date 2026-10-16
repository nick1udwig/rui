"""Independent complete-record decoder for accepted native trace comparison.

This is a bounded conformance oracle, not an untrusted network client. It checks
record identity/version/retirement and references, and compares every decoded
field to Rust; core validation remains the separate authoritative negative gate.
"""
import copy
import hashlib

try:
    from .reference import fields, one, many, number, double, string
    from .scene_reference import primitive, rect, point, size
except ImportError:
    from reference import fields, one, many, number, double, string
    from scene_reference import primitive, rect, point, size

KINDS = ['Surface', 'Scene', 'Semantic', 'Text', 'Binding', 'Session']


def enum(message, tag, names):
    index = number(message, tag)
    if not 1 <= index <= len(names):
        raise ValueError('unsupported enum')
    return names[index - 1]


def child(message, tag, decoder, optional=False):
    raw = one(message, tag, 2)
    if raw is None:
        if optional:
            return None
        raise ValueError('missing required message')
    return decoder(fields(raw))


def children(message, tag, decoder):
    return [decoder(fields(raw)) for raw in many(message, tag, 2)]


def ref(message):
    return {'kind': enum(message, 1, KINDS), 'id': number(message, 2)}


def affine(message):
    return {key: double(message, tag) for tag, key in enumerate(('xx', 'yx', 'xy', 'yy', 'tx', 'ty'), 1)}


def signed(value):
    return (value >> 1) ^ -(value & 1)


def typed(message):
    present = [tag for tag in range(1, 5) if tag in message]
    if len(present) != 1:
        raise ValueError('typed value variant')
    tag = present[0]
    return {['Text', 'Boolean', 'Number', 'Integer'][tag - 1]:
            {1: lambda: string(message, 1), 2: lambda: bool(number(message, 2)),
             3: lambda: double(message, 3), 4: lambda: signed(number(message, 4))}[tag]()}


def collection(message):
    return {'version': number(message, 1), 'items': children(message, 2, ref),
            'total_count': number(message, 3) if 3 in message else None,
            'materialization': enum(message, 4, ['Full', 'Partial']),
            'selected_items': children(message, 5, ref)}


def cursor(message):
    if (3 in message) == (4 in message):
        raise ValueError('cursor appearance variant')
    appearance = ({'Shape': enum(message, 3, ['Default', 'Text', 'Pointer', 'Hidden'])}
                  if 3 in message else {'Outline': list(one(message, 4, 2))})
    return {'position': child(message, 1, point), 'hotspot': child(message, 2, point), 'appearance': appearance}


def record(raw):
    wrapper = fields(raw)
    variants = [tag for tag in wrapper if tag < 128]
    if len(variants) != 1 or variants[0] not in range(1, 7):
        raise ValueError('unsupported record variant')
    kind = KINDS[variants[0] - 1]
    node = fields(one(wrapper, variants[0], 2))
    result = {'id': number(node, 1), 'version': number(node, 2)}
    if not result['id'] or not result['version']:
        raise ValueError('zero record identity or clock')
    if kind == 'Surface':
        result.update(role=enum(node, 3, ['Toplevel', 'Popup', 'Tooltip', 'Shell']),
                      parent=child(node, 4, ref, True), desktop_rect=child(node, 5, rect),
                      content_size=child(node, 6, size), transform=child(node, 7, affine),
                      clip=child(node, 8, rect, True), opacity=double(node, 9),
                      stacking_order=signed(number(node, 10)), mapped=bool(number(node, 11)),
                      focused=bool(number(node, 12)), modal_for=child(node, 13, ref, True),
                      scene_root=child(node, 14, ref, True), semantic_root=child(node, 15, ref, True),
                      input_region=children(node, 16, rect), routing_token=list(one(node, 17, 2, b'')),
                      visual_coverage=enum(node, 18, ['Complete', 'Partial', 'Unsupported']),
                      semantic_coverage=enum(node, 19, ['Rich', 'Partial', 'Opaque']),
                      capture_consistency=enum(node, 20, ['Coherent', 'Observed']),
                      source_capture_tokens=[list(raw) for raw in many(node, 21, 2)],
                      unsupported_features=[raw.decode() for raw in many(node, 22, 2)])
    elif kind == 'Scene':
        result.update(children=children(node, 3, ref), transform=child(node, 4, affine),
                      clip=child(node, 5, rect, True), opacity=double(node, 6), primitive=primitive(node),
                      resources=sorted([list(raw) for raw in set(many(node, 20, 2))]))
    elif kind == 'Semantic':
        result.update(role=enum(node, 3, ['Root', 'Group', 'Button', 'Checkbox', 'TextField', 'Label', 'List', 'ListItem', 'CustomCanvas']),
                      label=string(node, 4), description=string(node, 5), value=child(node, 6, typed, True),
                      states={enum(state, 1, ['Disabled', 'Checked', 'Selected', 'Expanded', 'ReadOnly']):
                              enum(state, 2, ['False', 'True', 'Unknown']) for state in map(fields, many(node, 7, 2))},
                      children=children(node, 8, ref),
                      relations=children(node, 9, lambda rel: {'kind': enum(rel, 1, ['LabelFor', 'LabelledBy', 'Controls', 'Describes', 'MemberOf']), 'targets': children(rel, 2, ref)}),
                      actions=children(node, 10, lambda action: {'id': string(action, 1, ''), 'schema_version': number(action, 2)}),
                      text=child(node, 11, ref, True), collection=child(node, 12, collection, True),
                      bindings=children(node, 13, ref), semantic_group=child(node, 14, ref, True),
                      reading_order=number(node, 15) if 15 in node else None,
                      sensitivity=enum(node, 16, ['Public', 'Sensitive', 'Redacted']),
                      completeness=enum(node, 17, ['Full', 'Incomplete', 'Opaque']))
        if 128 in node:
            result['browser_input_type'] = string(node, 128)
    elif kind == 'Text':
        result.update(text_version=number(node, 3), status=enum(node, 4, ['Available', 'Unavailable', 'Redacted']),
                      content=string(node, 5), selection=child(node, 6, lambda r: {'anchor': number(r, 1), 'focus': number(r, 2)}, True),
                      editable=bool(number(node, 7)), selectable=bool(number(node, 8)))
    elif kind == 'Binding':
        result.update(surface=child(node, 3, ref), semantics=children(node, 4, ref), scenes=children(node, 5, ref),
                      interaction_region=child(node, 6, rect, True))
    else:
        result.update(canvas=child(node, 3, rect), cursor=child(node, 4, cursor, True),
                      focus=child(node, 5, ref, True), focus_surface=child(node, 6, ref, True), focus_generation=number(node, 7))
    return {kind: result}


class FullReplica:
    def __init__(self):
        self.revision, self.records, self.retired, self.history = 0, {}, set(), {}

    def apply(self, kind, raw):
        message = fields(raw)
        revision = number(message, 1 if kind == 'snapshot' else 2)
        digest = hashlib.sha256(raw).digest()
        if revision <= self.revision:
            if self.history.get(revision) == (kind, digest):
                return self.state()
            raise ValueError('conflicting old publication')
        records = {} if kind == 'snapshot' else copy.deepcopy(self.records)
        retired = self.retired.copy()
        touched = set()

        def put(raw):
            wrapper = record(raw)
            name, node = next(iter(wrapper.items()))
            identity = str(node['id'])
            if identity in touched or int(identity) in retired:
                raise ValueError('duplicate or retired object')
            old = self.records.get(identity)
            if old:
                old_kind, previous = next(iter(old.items()))
                if old_kind != name or node['version'] < previous['version'] or (node['version'] == previous['version'] and old != wrapper):
                    raise ValueError('invalid object clock')
            touched.add(identity)
            records[identity] = wrapper

        if kind == 'snapshot':
            for raw_record in many(message, 2, 2):
                put(raw_record)
            retired.update(int(identity) for identity in self.records.keys() - records.keys())
        elif kind == 'transaction':
            if number(message, 1) != self.revision or revision != self.revision + 1:
                raise ValueError('transaction base mismatch')
            for raw_mutation in many(message, 3, 2):
                mutation = fields(raw_mutation)
                variants = [tag for tag in mutation if tag < 128]
                if variants == [1]:
                    put(one(mutation, 1, 2))
                elif variants == [2]:
                    target = child(mutation, 2, ref)
                    identity = str(target['id'])
                    if identity in touched or identity not in records or target['kind'] not in records[identity]:
                        raise ValueError('invalid deletion')
                    touched.add(identity)
                    retired.add(target['id'])
                    del records[identity]
                else:
                    raise ValueError('unsupported mutation')
        else:
            raise ValueError('unsupported publication')
        if len(records) > 16384 or len(retired) > 65536:
            raise ValueError('oracle record ceiling')

        def references(value):
            if isinstance(value, dict):
                if set(value) == {'kind', 'id'}:
                    if value['kind'] not in records.get(str(value['id']), {}):
                        raise ValueError('dangling or wrong-kind reference')
                else:
                    for nested in value.values():
                        references(nested)
            elif isinstance(value, list):
                for nested in value:
                    references(nested)
        references(records)
        self.records, self.retired, self.revision = records, retired, revision
        self.history[revision] = kind, digest
        while len(self.history) > 4096:
            del self.history[min(self.history)]
        return self.state()

    def state(self):
        return {'revision': self.revision, 'records': self.records, 'retired': sorted(self.retired)}
