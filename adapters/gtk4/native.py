"""Private cooperating source dispatcher, never a peer command endpoint.

Only the Rust host owns this inherited socket. Its one common core ledger has
already authenticated/admitted each dispatch. All source checks run again on
the GTK main-loop thread immediately before any native call. There is no Python
command ledger and no input to an existing personal desktop session.
"""
import json
import os
import struct
import time

from gi.repository import GLib, Gtk

from adapters.gtk4.state import NativeAction, SourceRejected, editable_text


MAX_REQUEST = 128 << 10
MAX_REPLY = 8 << 10


def decode_request(body):
    # Reserve the fixed read buffer before touching peer-length-controlled
    # bytes; bound JSON nesting/tokens before Python allocates object trees.
    if not 0 < len(body) <= MAX_REQUEST:
        raise ValueError('private-request-ceiling')
    depth = tokens = 0
    quoted = escaped = False
    for byte in body:
        if quoted:
            if escaped:
                escaped = False
            elif byte == 92:
                escaped = True
            elif byte == 34:
                quoted = False
        elif byte == 34:
            quoted = True
        elif byte in (91, 123):
            depth += 1
            tokens += 1
            if depth > 8:
                raise ValueError('private-request-depth')
        elif byte in (93, 125):
            depth -= 1
        elif byte in (44, 58):
            tokens += 1
        if tokens > 128:
            raise ValueError('private-request-tokens')
    value = json.loads(body)
    if not isinstance(value, dict) or type(value.get('request_id')) is not int or value['request_id'] <= 0:
        raise ValueError('private-request-shape')
    return value


class NativeDispatcher:
    def __init__(self, corpus, *, fixture_hooks=False, whole=None):
        self.corpus = corpus
        self.whole = whole
        self.source_generation = int(os.environ.get("RUI_PRIVATE_SOURCE_GENERATION", "1"))
        if not 1 <= self.source_generation <= 5:
            raise ValueError("private-source-generation-ceiling")
        corpus.source.private_generation = self.source_generation
        self.fixture_hooks = fixture_hooks
        self.buffer = bytearray(MAX_REQUEST + 4)
        self.used = 0
        self.pending = (0, 'ready')
        self.native_dispatches = 0
        self.native_callbacks = 0
        self.race_next = None
        self.fail_after_dispatch = None
        self.control = None
        self.delay_until_expired = False
        self.closed = False
        self.current_artifact = None
        self.promised_artifact = None
        self.artifacts = {}
        self.original_capture = corpus.capture.on_capture
        if whole is None:
            corpus.capture.on_capture = self.captured
        else:
            whole.consumer = self.whole_captured
            self.capture_memory = whole.budget.reserve(8 << 20)
            GLib.timeout_add(400, self.request_capture)
            for capture in whole.captures.values():
                capture.connect('map', self.observed_callback)
                capture.connect('unmap', self.observed_callback)
        for widget, signal in [(corpus.button, 'clicked'), (corpus.entry, 'changed'),
                               (corpus.list, 'row-selected'), (corpus.window, 'notify::focus-widget')]:
            widget.connect(signal, self.observed_callback)
        os.set_blocking(0, False)
        GLib.io_add_watch(0, GLib.IO_IN | GLib.IO_HUP | GLib.IO_ERR, self.readable)
        # A source/pipe stall terminates this controlled process rather than
        # keeping an admission ambiguous indefinitely. The host has its own
        # deadline and never automatically retries a possibly dispatched call.
        GLib.timeout_add_seconds(30, self.watchdog)

    def observed_callback(self, *_):
        self.native_callbacks += 1

    def watchdog(self):
        self.close()
        self.corpus.app.quit()
        return False

    def close(self):
        self.closed = True
        if self.whole is not None:
            self.whole.barrier.close()
            self.capture_memory.close()

    def reply(self, request_id, status, *, artifact=None, reason=None):
        body = json.dumps({'request_id': request_id, 'source_generation': self.source_generation, 'status': status, 'artifact': artifact,
                           'reason': reason, 'native_dispatches': self.native_dispatches,
                           'native_callbacks': self.native_callbacks}, separators=(',', ':')).encode()
        if len(body) > MAX_REPLY:
            raise ValueError('private-reply-ceiling')
        # Replies are below the private socket's send buffer. No text arguments
        # are echoed into diagnostics; only the actual capture owns source text.
        view = memoryview(struct.pack('>I', len(body)) + body)
        while view:
            count = os.write(0, view)
            view = view[count:]

    def captured(self, capture, frame):
        if self.original_capture:
            self.original_capture(capture, frame)
        label = f"{capture.surface_key}-{frame['revision']:04d}"
        files = {label + '.json', label + '.bin'} | {digest + '.rvp' for digest in frame['resources']}
        if 'local_reference' in frame:
            files.update({label + '.local.png', label + '.local.bgra'})
        self.artifacts[label + '.json'] = files
        if self.pending is not None:
            request_id, status = self.pending
            self.pending = None
            self.promised_artifact = label + '.json'
            self.reply(request_id, status, artifact=self.promised_artifact)
            capture.command_cause = None
        self.prune_artifacts()

    def whole_captured(self, whole, frame, label):
        files = {label + '.json', label + '.bin', label + '.local.png', label + '.local.bgra'} | {digest + '.rvp' for digest in frame['resources']}
        for surface in frame['surfaces']:
            if surface['mapped']:
                prefix = surface['local_reference_label']
                files.update(prefix + suffix for suffix in ('.json', '.bin', '.local.png', '.local.bgra'))
        self.artifacts[label + '.json'] = files
        if self.pending is None:
            raise ValueError('whole-source-capture-without-owner')
        request_id, status = self.pending
        self.pending = None
        self.promised_artifact = label + '.json'
        self.reply(request_id, status, artifact=self.promised_artifact)
        whole.command_cause = None
        self.prune_artifacts()

    def request_capture(self):
        if self.whole is None:
            self.corpus.capture.queue_draw()
        elif not self.closed:
            try:
                self.whole.barrier.begin()
            except Exception as error:
                # No causal handoff occurred. The Rust owner records UNKNOWN
                # after an attempted action, and never redispatches on timeout.
                self.whole.failed(error)
                self.close()
        return False

    def prune_artifacts(self):
        owners = {self.current_artifact, self.promised_artifact} - {None}
        keep = set().union(*(files for artifact, files in self.artifacts.items() if artifact in owners))
        obsolete = set().union(*(files for artifact, files in self.artifacts.items() if artifact not in owners)) - keep
        for name in obsolete:
            (self.corpus.output / name).unlink(missing_ok=True)
        self.artifacts = {name: files for name, files in self.artifacts.items() if name in owners}
        if self.whole is not None:
            quota = self.whole.source.artifact_quota
            for path in tuple(quota.files):
                if path.name not in keep:
                    quota.discard(path)
        # Two retained capture closures, plus the just-written unclaimed frame
        # before pruning: each has <=32MiB vector bodies, <=2MiB metadata/wire,
        # <=4MiB BGRA and <=4MiB+64KiB PNG. Never a sustained artifact history.
        if sum((self.corpus.output / name).stat().st_size for name in keep) > 128 << 20:
            raise ValueError('private-artifact-retention-ceiling')

    def readable(self, _fd, condition):
        if condition & (GLib.IO_HUP | GLib.IO_ERR):
            self.close()
            self.corpus.app.quit()
            return False
        try:
            body = os.read(0, len(self.buffer) - self.used)
            if not body:
                self.close()
                self.corpus.app.quit()
                return False
            self.buffer[self.used:self.used + len(body)] = body
            self.used += len(body)
            if self.used >= 4:
                length = struct.unpack_from('>I', self.buffer)[0]
                if not 0 < length <= MAX_REQUEST:
                    raise ValueError('private-request-ceiling')
                if self.used >= length + 4:
                    if self.used != length + 4 or self.pending is not None:
                        raise ValueError('private-request-overlap')
                    request = decode_request(memoryview(self.buffer)[4:4 + length].tobytes())
                    self.used = 0
                    self.handle(request)
        except (ValueError, KeyError, TypeError, OSError):
            self.close()
            self.corpus.app.quit()
            return False
        return True

    def handle(self, request):
        request_id = request['request_id']
        kind = request['kind']
        if kind == 'consumed':
            if request['artifact'] != self.promised_artifact:
                raise ValueError('private-capture-owner-mismatch')
            self.current_artifact = self.promised_artifact
            self.promised_artifact = None
            self.prune_artifacts()
            self.reply(request_id, 'consumed')
            return
        if self.promised_artifact is not None:
            raise ValueError('private-unconsumed-capture')
        if kind == 'control':
            self.control = (request['lease_id'], request['generation'], request['deadline_ns'])
            self.reply(request_id, 'control')
            return
        if kind == 'capture':
            self.pending = (request_id, 'capture')
            self.request_capture()
            return
        if kind == 'fixture' and self.fixture_hooks:
            operation = request['operation']
            if operation == 'race-edit':
                self.race_next = 'edit'
            elif operation == 'race-destroy':
                self.race_next = 'destroy'
            elif operation == 'race-visual':
                self.race_next = 'visual'
            elif operation == 'race-model':
                self.race_next = 'model'
            elif operation == 'race-source-generation':
                self.race_next = 'source-generation'
            elif operation == 'interrupt-whole-capture' and self.whole is not None:
                self.whole.interrupt = True
            elif operation in ('lose-after-dispatch', 'stall-after-dispatch', 'uncorrelated-capture', 'crash-after-dispatch'):
                self.fail_after_dispatch = operation
            elif operation == 'delay-until-expired':
                self.delay_until_expired = True
            elif operation == 'disable-button':
                self.corpus.button.set_sensitive(False)
            elif operation == 'enable-button':
                self.corpus.button.set_sensitive(True)
            elif operation == 'destroy-button':
                self.corpus.button.set_visible(False)
                self.corpus.registry.retire('button')
            elif operation == 'destroy-surface':
                self.pending = (request_id, 'fixture-capture')
                self.corpus.capture.retire_source()
                self.corpus.window.destroy()
                return
            else:
                raise ValueError('unsupported-fixture-operation')
            self.reply(request_id, 'fixture')
            return
        if kind != 'semantic':
            raise ValueError('unsupported-private-operation')
        guard = request['guard']
        if self.race_next == 'source-generation':
            # Trusted negative-control hook injects a delayed old-source guard
            # after core admission, at the final serialized native boundary.
            guard['source_generation'] = self.source_generation - 1
            self.race_next = None
        if self.delay_until_expired:
            self.delay_until_expired = False
            delay = max(1, (guard['deadline_ns'] - time.monotonic_ns()) // 1_000_000 + 50)
            GLib.timeout_add(delay, lambda: (self.handle(request), False)[1])
            return
        if self.race_next == 'edit':
            self.corpus.entry.set_text('local intervening source edit')
            self.race_next = None
        elif self.race_next == 'destroy':
            self.corpus.button.set_visible(False)
            self.corpus.registry.retire('button')
            self.race_next = None
        elif self.race_next == 'visual':
            self.corpus.canvas.queue_draw()
            self.corpus.source.changed()
            self.race_next = None
        elif self.race_next == 'model':
            self.corpus.unmaterialize_row()
            self.race_next = None
        try:
            if guard['source_generation'] != self.source_generation:
                raise SourceRejected('retired-private-source-generation')
            if self.control != (guard['lease_id'], guard['control_generation'], guard['deadline_ns']):
                raise SourceRejected('retired-native-control-generation')
            if time.monotonic_ns() >= guard['deadline_ns']:
                if self.fixture_hooks:
                    (self.corpus.output / 'native-expiry-receipt.json').write_text(json.dumps({'native_dispatches': self.native_dispatches, 'reason': 'lease-expired-at-native-dispatch'}))
                raise SourceRejected('lease-expired-at-native-dispatch')
            key, widget = self.corpus.registry.resolve_semantic(guard['incarnation'], guard['surface_id'], guard['target_id'])
            owner = self.corpus.registry.widgets[key][1]
            if self.whole is not None and self.whole.modal_capture.get_mapped() and owner != 'modal':
                raise SourceRejected('blocked-by-native-modal')
            if self.whole is None and self.corpus.popup_capture.get_mapped():
                # This semantic fixture projects only main content. A native
                # transient blocks it until a whole-view projection is available.
                raise SourceRejected('blocked-by-native-popup-modality')
            capture = self.corpus.registry.surfaces[owner]
            # Actual native reads catch edits/selection/materialization changes
            # even when GTK has not submitted its next visual frame yet.
            capture.source.focus_changed(capture.surface_key, capture.focus_reader())
            current = self.corpus.source.semantics(capture.semantic_root, capture.frames)
            semantic = next(node for node in current if node['id'] == guard['target_id'])
            if semantic['version'] != guard['target_version']:
                raise SourceRejected('stale-target-clock')
            if guard.get('text_version') is not None and semantic.get('text', {}).get('text_version') != guard['text_version']:
                raise SourceRejected('stale-text-clock')
            if guard.get('model_version') is not None:
                collections = [node['collection_model'] for node in current if node.get('collection_model') and guard['target_id'] in node['collection_model']['items']]
                own = semantic.get('collection_model')
                actual = own or (collections[0] if len(collections) == 1 else None)
                if actual is None or actual['version'] != guard['model_version']:
                    raise SourceRejected('stale-model-clock')
            if guard.get('focus_generation') is not None:
                if self.whole is None:
                    current_focus = self.corpus.source._focus.get(owner, (None, 1))[1]
                else:
                    facts = self.whole.oracle()
                    if facts['focus'] != self.whole.previous_focus:
                        raise SourceRejected('stale-native-aggregate-focus')
                    current_focus = self.whole.focus_generation
                if current_focus != guard['focus_generation']:
                    raise SourceRejected('stale-focus-clock')
            action, arguments = request['action'], request['arguments']
            if action not in semantic['actions']:
                raise SourceRejected('unsupported-native-action')
            self.validate_arguments(key, widget, action, arguments)
            if time.monotonic_ns() >= guard['deadline_ns']:
                raise SourceRejected('lease-expired-at-native-dispatch')
        except (SourceRejected, StopIteration) as error:
            self.reply(request_id, 'rejected', reason=str(error) or 'retired-source-object')
            return
        # Every prior check and the native calls share this GTK main-loop turn.
        # No peer callback/crawl can replace the source object between them.
        self.native_dispatches += 1
        capture.command_cause = {'private_request_id': request_id, 'source_generation': self.source_generation, 'authority_gate': 'core-ledger-admitted'}
        if self.whole is not None:
            self.whole.command_cause = capture.command_cause
        self.pending = (request_id, 'applied')
        try:
            self.apply_native(key, widget, action, arguments)
        except Exception:
            # Once native execution started, any unexpected source failure is
            # ambiguous. Close the pipe; the Rust ledger records UNKNOWN.
            self.close()
            self.corpus.app.quit()
            return
        if self.fail_after_dispatch:
            (self.corpus.output / 'native-fault-receipt.json').write_text(json.dumps({
                'native_dispatches': self.native_dispatches, 'native_callbacks': self.native_callbacks,
                'fault': self.fail_after_dispatch, 'scope': 'local-negative-control-never-applied-evidence'}))
            capture.command_cause = None
            if self.fail_after_dispatch == 'uncorrelated-capture':
                self.fail_after_dispatch = None
                self.corpus.capture.queue_draw()
                return
            self.pending = None
            if self.fail_after_dispatch == 'crash-after-dispatch':
                # Deliberate isolated process-loss fault, after an actual native
                # callback and before any causal capture acknowledgement.
                import signal
                os.kill(os.getpid(), signal.SIGKILL)
            if self.fail_after_dispatch == 'lose-after-dispatch':
                os.close(0)
                self.corpus.app.quit()
            return
        if self.whole is None:
            self.request_capture()
        else:
            # Mapping/layout/focus notifications settle before reading the
            # whole boundary; only fresh subsequent snapshots become candidates.
            GLib.timeout_add(20, self.request_capture)

    def validate_arguments(self, key, widget, action, arguments):
        if action in ('ReplaceText', 'SetSelection'):
            if not isinstance(widget, Gtk.Entry) or not widget.get_editable():
                raise SourceRejected('native-text-not-editable')
            count = len(widget.get_text())
            if action == 'ReplaceText':
                start, end, text = arguments['start'], arguments['end'], arguments['text']
                if not 0 <= start <= end <= count or len(text.encode()) > 16 << 10 or '\0' in text:
                    raise SourceRejected('native-text-range')
                result = widget.get_text()[:start] + text + widget.get_text()[end:]
                if len(result.encode()) > 16 << 10:
                    raise SourceRejected('native-text-ceiling')
            elif not all(0 <= arguments[name] <= count for name in ('anchor', 'focus')):
                raise SourceRejected('native-selection-range')
        elif action == 'Activate' and not isinstance(widget, (Gtk.Button, Gtk.MenuButton, Gtk.ListBoxRow, NativeAction)):
            raise SourceRejected('unsupported-native-activation')
        elif action == 'ScrollTo' and key != 'list':
            raise SourceRejected('unsupported-native-scroll')
        elif action not in ('Activate', 'Focus', 'ScrollTo'):
            raise SourceRejected('unsupported-native-action')

    def apply_native(self, key, widget, action, arguments):
        if action == 'Activate':
            if isinstance(widget, NativeAction):
                widget.activate()
            elif isinstance(widget, Gtk.MenuButton):
                if widget.get_popover().get_mapped():
                    widget.popdown()
                else:
                    widget.popup()
            elif isinstance(widget, Gtk.Button):
                widget.emit('clicked')
            else:
                widget.get_parent().select_row(widget)
        elif action == 'Focus':
            if not widget.grab_focus():
                raise RuntimeError('native focus declined after dispatch')
        elif action == 'ReplaceText':
            content = widget.get_text()
            result = content[:arguments['start']] + arguments['text'] + content[arguments['end']:]
            widget.set_text(result)
            # Programmatic set_text resets GTK's cursor. Interactive insertion
            # has an explicit scalar caret at the end of the inserted string.
            (widget.get_delegate() or widget).set_position(arguments['start'] + len(arguments['text']))
            if widget.get_text() != result:
                raise RuntimeError('native text differs after dispatch')
        elif action == 'SetSelection':
            delegate = widget.get_delegate() or widget
            anchor, focus = arguments['anchor'], arguments['focus']
            # GtkText movement skips grapheme cursor boundaries. GtkEditable
            # selection ranges count Unicode scalars, including boundaries
            # between a base and combining mark; use that native API directly.
            delegate.select_region(anchor, focus)
            if editable_text(widget)['selection'] != [anchor, focus]:
                raise RuntimeError('native scalar selection differs after dispatch')
        elif action == 'ScrollTo':
            scroll = widget.get_ancestor(Gtk.ScrolledWindow)
            for adjustment, value in [(scroll.get_hadjustment(), arguments['x']), (scroll.get_vadjustment(), arguments['y'])]:
                adjustment.set_value(value)
