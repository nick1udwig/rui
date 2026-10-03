"""Narrow desktop composition evidence check, using the independent full decoder.

Run alongside conformance.network_audit for exact scope, resources and Rust
replay. This check adds native stage ordering, presentation, scalar edits and
the declared GTK clock/target-Surface cause contract; it is not a network client.
"""
from collections import Counter
import hashlib
import json
from pathlib import Path

from conformance.full_reference import FullReplica
from conformance.network_audit import MAX_FILE, MAX_PACKET, MAX_PUBLICATION, assemble, complete, identity
from conformance.reference import fields, many, number, one, string

CLOCK_PROFILE = 'text.clock.gtk-editable4224.v1'


def packets(path):
    if path.stat().st_size > MAX_FILE:
        raise ValueError('composition trace ceiling')
    scope = None
    with path.open('rb') as stream:
        for index in range(32769):
            line = stream.readline(2 * MAX_PACKET + 8193)
            if not line:
                break
            if index == 32768 or len(line) > 2 * MAX_PACKET + 8192:
                raise ValueError('composition trace record ceiling')
            entry = json.loads(line)
            if entry['record'] == 'scope':
                if scope is not None:
                    raise ValueError('this evidence requires one complete original scope')
                scope = tuple(bytes.fromhex(entry[key]) for key in ('session_id', 'epoch', 'view_id'))
            elif entry['record'] == 'packet':
                raw = bytes.fromhex(entry['exact_packet_hex'])
                if len(raw) != entry['wire_bytes'] or len(raw) > MAX_PACKET or entry['sha256'] != hashlib.sha256(raw).hexdigest():
                    raise ValueError('composition packet length/digest')
                packet = fields(raw)
                if scope != tuple(identity(packet, tag) for tag in (1, 2, 3)):
                    raise ValueError('composition packet scope')
                yield entry['direction'], number(packet, 5), fields(one(packet, 6, 2, b'')), scope


def focused(state):
    sessions = [value['Session'] for value in state['records'].values() if 'Session' in value]
    if len(sessions) != 1 or not sessions[0]['focus'] or not sessions[0]['focus_surface']:
        raise ValueError('composition needs a published focused target and Surface')
    session = sessions[0]
    target = state['records'][str(session['focus']['id'])]['Semantic']
    text = state['records'][str(target['text']['id'])]['Text']
    surface = state['records'][str(session['focus_surface']['id'])]['Surface']
    if (text['status'] != 'Available' or not text['editable'] or not text['selection']
            or not surface['mapped'] or surface['capture_consistency'] != 'Coherent'):
        raise ValueError('composition source context unavailable')
    return {'target': target['id'], 'text': text, 'surface': surface, 'focus_generation': session['focus_generation']}


def same_context(left, right):
    return (left['target'], left['focus_generation'], left['text']) == (right['target'], right['focus_generation'], right['text'])


def verify_controller(path):
    replica, transfer, state = FullReplica(), None, None
    presented, lease, active, pending = 0, None, None, None
    receipts, retired, stages, commits = {}, set(), Counter(), []
    profiles, modes, actions = None, set(), set()
    semantic_commands = 0

    def confirm():
        nonlocal pending
        if pending is None or pending.get('result') is None or state['revision'] <= pending['before_revision']:
            return
        current = focused(state)
        result, before = pending['result'], pending['context']
        text, old = current['text'], before['text']
        after, caret = pending['after'], pending['caret']
        content_delta = int(bool(old['content'])) + int(bool(after))
        record_delta = content_delta + int(caret > 0)
        if (result['revision'] is not None and state['revision'] < result['revision']):
            return
        if (current['target'] != before['target'] or current['focus_generation'] != before['focus_generation']
                or text != dict(old, content=after, selection={'anchor': caret, 'focus': caret},
                                text_version=old['text_version'] + content_delta, version=old['version'] + record_delta)
                or list(result['cause'].encode()) not in current['surface']['source_capture_tokens']):
            raise ValueError('composition edit/clock/target-Surface causal evidence differs')
        commits.append({'sequence': pending['sequence'], 'revision': state['revision'],
                        'target': current['target'], 'content': after, 'caret': caret,
                        'text_version': text['text_version'], 'text_record_version': text['version'],
                        'source_commit': result['cause']})
        pending = None

    for direction, kind, body, scope in packets(path):
        host = direction == 'host-to-client'
        if host and kind == 2:
            profiles = {raw.decode() for raw in many(body, 2, 2)}
        elif host and kind == 4:
            modes = {raw.decode() for raw in many(body, 5, 2)}
        elif host and kind == 41:
            lease = identity(body, 1), number(body, 2)
            actions = {raw.decode() for raw in many(body, 4, 2)}
        elif host and kind in (10, 11):
            if transfer is not None or not 0 < number(body, 4) <= MAX_PUBLICATION or not 0 < number(body, 5) <= 4096:
                raise ValueError('composition publication ceiling')
            transfer = {'id': identity(body, 1), 'count': number(body, 5), 'total': number(body, 4),
                        'received': 0, 'chunks': [], 'kind': 'snapshot' if kind == 10 else 'transaction'}
        elif host and kind == 12:
            if transfer is None:
                raise ValueError('composition publication chunk without begin')
            assemble(transfer, identity(body, 1), number(body, 2), one(body, 3, 2))
        elif host and kind == 13:
            if transfer is None:
                raise ValueError('composition publication commit without begin')
            state = replica.apply(transfer['kind'], complete(transfer))
            transfer = None
            if active is not None and pending is None and not same_context(active['context'], focused(state)):
                raise ValueError('preedit altered authoritative source context')
            confirm()
        elif not host and kind == 31:
            presented = number(body, 1)
        elif not host and kind == 51:
            raise ValueError('composing physical keys forwarded as RawInput')
        elif not host and kind == 50:
            # This scenario only selects the source-owned editable by Focus.
            # Any ReplaceText would bypass the advertised composition family.
            if string(body, 8) != 'Focus':
                raise ValueError('semantic text fallback in wire composition trace')
            semantic_commands += 1
        elif not host and kind == 52:
            if (profiles is not None and CLOCK_PROFILE not in profiles
                    or 'text.client.v1' not in modes or 'TextComposition' not in actions):
                raise ValueError('composition profile/mode/action was not negotiated')
            if state is None or presented != state['revision'] or receipts or pending is not None:
                raise ValueError('composition dispatched before source confirmation and actual Presented')
            header = fields(one(body, 1, 2))
            if ((identity(header, 1), identity(header, 2)) != scope[1:]
                    or (identity(header, 5), number(header, 6)) != lease
                    or number(header, 7) != state['revision']):
                raise ValueError('composition header does not own the current painted grant')
            context, identifier = focused(state), identity(body, 3)
            if (number(body, 2), number(body, 4), number(body, 5)) != (
                    context['target'], context['focus_generation'], context['text']['text_version']):
                raise ValueError('composition target/focus/content clocks differ')
            variants = [tag for tag in (6, 7, 8, 9) if tag in body]
            if len(variants) != 1:
                raise ValueError('composition stage variant')
            stage = variants[0]
            operation, sequence = fields(one(body, stage, 2)), number(header, 4)
            stages[stage] += 1
            if stage == 6:
                selection = context['text']['selection']
                interval = min(selection.values()), max(selection.values())
                if active is not None or identifier in retired or (number(operation, 1), number(operation, 2)) != interval:
                    raise ValueError('composition Begin identity/scalar range')
                retired.add(identifier)
                if len(retired) > 128:
                    raise ValueError('composition identity ceiling')
                active = {'id': identifier, 'context': context, 'range': interval}
            else:
                if active is None or active['id'] != identifier or not same_context(active['context'], context):
                    raise ValueError('composition stage uses a retired/changed native owner')
                text = string(operation, 1, '')
                if len(text.encode()) > 16384:
                    raise ValueError('composition scalar text ceiling')
                if stage == 7:
                    selection = fields(one(operation, 2, 2))
                    if not 0 <= number(selection, 1) <= number(selection, 2) <= len(text):
                        raise ValueError('composition preedit scalar selection')
                elif stage == 8:
                    start, end = active['range']
                    before = context['text']['content']
                    pending = {'sequence': sequence, 'context': context, 'before_revision': state['revision'],
                               'after': before[:start] + text + before[end:], 'caret': start + len(text)}
                    active = None
                elif stage == 9:
                    active = None
            receipts[sequence] = stage
        elif host and kind == 54 and number(body, 1) in receipts:
            sequence = number(body, 1)
            stage = receipts.pop(sequence)
            status, cause = string(body, 2), string(body, 4, '')
            revision = number(body, 5) if 5 in body else None
            if stage == 8:
                if status != 'Applied' or not cause or pending is None or pending['sequence'] != sequence:
                    raise ValueError('composition Commit lacks terminal causal Applied')
                pending['result'] = {'cause': cause, 'revision': revision}
                confirm()
            elif status != 'Dispatched' or cause or revision is not None:
                raise ValueError('transient preedit invented Applied/source evidence')
    if transfer or receipts or pending or active or stages != Counter({6: 5, 7: 12, 8: 4, 9: 1}) or semantic_commands != 1:
        raise ValueError('incomplete native composition scenario/stages')
    if [(commit['content'], commit['caret']) for commit in commits] != [('a', 1), ('aé', 2), ('aéx', 3), ('aéxy', 4)]:
        raise ValueError('native scalar commits lost order/exactly-once caret')
    return {'scope': 'Narrow native desktop composition checks; full network/resource audit remains separate',
            'handshake_scope': 'Post-auth recorder excludes ServerHello; negotiated clock profile is enforced by the runtime and checked separately in native probe evidence.',
            'begins': stages[6], 'updates': stages[7], 'commits': commits, 'cancels': stages[9],
            'physical_composing_batches': 0, 'semantic_replace_text_fallbacks': 0,
            'native_batch_overlap': 'Not measured by the native batch; portable CompositionQueue regression proves queued overlap.'}


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('trace', type=Path)
    args = parser.parse_args()
    print(json.dumps(verify_controller(args.trace), indent=2, sort_keys=True))


if __name__ == '__main__':
    main()
