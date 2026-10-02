"""Exact typed semantic/text vectors through independent Python and Rust reducers."""
import json
from pathlib import Path
import subprocess

from conformance.semantic_client import SemanticReplica, ProtocolError, blob, scalar
from conformance.test_semantic_client import node, ref, snapshot

ROOT = Path(__file__).resolve().parent.parent


def text_model(version=1, clock=1, content='A😀e\u0301אב', anchor=1, focus=4):
    return blob(4, scalar(1, 10) + scalar(2, version) + scalar(3, clock) + scalar(4, 1)
                + blob(5, content) + blob(6, scalar(1, anchor) + scalar(2, focus))
                + scalar(7, 1) + scalar(8, 1))


def collection(version=1, order=(3, 4), selected=3):
    value = scalar(1, version) + scalar(3, 100) + scalar(4, 2)
    value += b''.join(blob(2, ref(3, identity)) for identity in order)
    return value + blob(5, ref(3, selected))


def vectors():
    actions = blob(10, blob(1, 'ReplaceText') + scalar(2, 1)) + blob(10, blob(1, 'SetSelection') + scalar(2, 1))
    root = node(1, role=1, label='Collection', children=(2, 3, 4), extra=blob(12, collection()))
    edit = node(2, role=5, label='Unicode', extra=blob(11, ref(4, 10)) + actions)
    # Exact unsigned IDs, negative signed value, false boolean presence, optional
    # metadata, composed/decomposed Unicode and nonempty scalar selection.
    first = snapshot([root, edit, node(3, role=8, extra=blob(6, scalar(4, 13))),
                      node(4, role=8, extra=blob(6, scalar(2, 0))), text_model()]) + scalar(999, 9)
    yield {'kind': 'snapshot', 'hex': first.hex()}
    changed = scalar(1, 1) + scalar(2, 2) + blob(3, blob(1, text_model(2, 2, 'Aé🦀Z', 1, 3)))
    yield {'kind': 'transaction', 'hex': changed.hex()}
    yield {'kind': 'transaction', 'hex': changed.hex()}
    listing = node(1, version=2, role=1, label='Collection', children=(2, 4, 3), extra=blob(12, collection(2, (4, 3), 4)))
    yield {'kind': 'transaction', 'hex': (scalar(1, 2) + scalar(2, 3) + blob(3, blob(1, listing))).hex()}
    # Stale typed clock with a superficially valid record clock must be atomic.
    yield {'kind': 'transaction', 'hex': (scalar(1, 3) + scalar(2, 4) + blob(3, blob(1, text_model(3, 2, 'wrong', 0, 0)))).hex()}
    selection = text_model(3, 2, 'Aé🦀Z', 0, 4)
    yield {'kind': 'transaction', 'hex': (scalar(1, 3) + scalar(2, 4) + blob(3, blob(1, selection))).hex()}


def normalized_python(replica):
    result = []
    for record in sorted(replica.records.values(), key=lambda record: record['id']):
        common = {'id': record['id'], 'version': record['version'], 'kind': record['kind']}
        if record['kind'] == 3:
            common.update(role=record['role'], label=record['label'], actions=record['actions'], states=record['states'],
                          children=[ref[1] for ref in record['children']], text=record.get('text_model'),
                          value=record.get('value'), value_kind=record.get('value_kind'), collection=record.get('collection'))
        elif record['kind'] == 4:
            common.update({key: record[key] for key in ('text_version', 'status', 'content', 'editable', 'selection')})
        else:
            raise ValueError('typed golden corpus only contains semantic/text records')
        result.append(common)
    return {'revision': replica.revision, 'retired': sorted(replica.retired), 'records': result}


def normalized_rust(state):
    roles = ['Root', 'Group', 'Button', 'Checkbox', 'TextField', 'Label', 'List', 'ListItem', 'CustomCanvas']
    result = []
    for wrapper in state['records'].values():
        kind, record = next(iter(wrapper.items()))
        common = {'id': record['id'], 'version': record['version'], 'kind': {'Semantic': 3, 'Text': 4}[kind]}
        if kind == 'Semantic':
            typed = record['value']
            collection = record['collection']
            common.update(role=roles.index(record['role']) + 1, label=record['label'],
                          actions={action['id']: action['schema_version'] for action in record['actions']},
                          states={['Disabled', 'Checked', 'Selected', 'Expanded', 'ReadOnly'].index(key) + 1:
                                  ['False', 'True', 'Unknown'].index(value) + 1 for key, value in record['states'].items()},
                          children=[ref['id'] for ref in record['children']], text=record['text']['id'] if record['text'] else None,
                          value=next(iter(typed.values())) if typed else None,
                          value_kind=['Text', 'Boolean', 'Number', 'Integer'].index(next(iter(typed))) + 1 if typed else None,
                          collection={'version': collection['version'], 'items': [ref['id'] for ref in collection['items']],
                                      'selected': [ref['id'] for ref in collection['selected_items']], 'total': collection['total_count'],
                                      'materialization': {'Full': 1, 'Partial': 2}[collection['materialization']]} if collection else None)
        else:
            common.update({key: record[key] for key in ('text_version', 'content', 'editable')})
            common['status'] = ['Available', 'Unavailable', 'Redacted'].index(record['status']) + 1
            common['selection'] = tuple(record['selection'][key] for key in ('anchor', 'focus')) if record['selection'] else None
        result.append(common)
    return {'revision': state['revision'], 'retired': state['retired'], 'records': sorted(result, key=lambda record: record['id'])}


def main():
    entries = list(vectors())
    golden = ROOT / 'conformance/golden/typed.jsonl'
    if [json.loads(line) for line in golden.read_text().splitlines()] != entries:
        raise AssertionError('typed golden bytes changed; review explicitly')
    run = subprocess.run([str(ROOT / 'target/debug/examples/replay'), '--full-state'],
                         input=''.join(json.dumps(item) + '\n' for item in entries), text=True,
                         capture_output=True, check=True, timeout=30)
    rust = [json.loads(line) for line in run.stdout.splitlines()]
    if len(rust) != len(entries):
        raise AssertionError('typed replay response count')
    replica = SemanticReplica()
    for index, (item, actual) in enumerate(zip(entries, rust)):
        try:
            replica.apply({'snapshot': 1, 'transaction': 2}[item['kind']], bytes.fromhex(item['hex']))
            accepted = True
        except ProtocolError:
            accepted = False
        if actual['accepted'] != accepted or normalized_rust(actual['state']) != normalized_python(replica):
            raise AssertionError(f'typed state differs at prefix {index}: Rust={actual}, Python={normalized_python(replica)}')
    print(f'Independent typed replay passed: {len(entries)} prefixes, exact Unicode/numeric/collection vectors')


if __name__ == '__main__':
    main()
