#!/usr/bin/env python3
"""Controlled GTK-thread transaction across explicitly registered surfaces.

Placement/stacking is owned by this isolated cooperating source, not observed
from an arbitrary window manager. Native source state and fresh actual snapshot
callbacks supply the transaction; native diagnostic pixels stay local.
"""
from collections import Counter
import argparse
import hashlib
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, GLib
from adapters.gtk4.capture import CaptureMixin, CaptureWidget, Exporter
from adapters.gtk4.transaction import CaptureTransaction, CaptureBudget, ArtifactQuota, BarrierRejected
from adapters.gtk4.state import editable_text
from adapters.sdk.producer import encode_snapshot, MAX_MANIFEST_BYTES
from fixtures.gtk.corpus import Corpus
import time
import cairo
import io


class TooltipCapture(CaptureMixin, Gtk.Popover):
    def __init__(self, source, output):
        super().__init__()
        self.set_has_arrow(False)
        self.set_autohide(False)
        self.set_child(Gtk.Label(label='Controlled native tooltip'))
        self.setup_capture(source, 'tooltip', 'tooltip-root', output, role=3)
        self.connect('unmap', lambda *_: self.publish_lifecycle())

    def do_snapshot(self, snapshot):
        started = time.monotonic_ns()
        revision = self.source_boundary()
        captured = Gtk.Snapshot()
        Gtk.Popover.do_snapshot(self, captured)
        self.capture_node(snapshot, captured.to_node(), revision, started)


class WholeView:
    def __init__(self, app, output, *, scripted=True):
        self.scripted = scripted
        self.consumer = None
        self.command_cause = None
        self.stage = 'cooperating-source-command'
        self.app, self.output = app, output
        self.corpus = Corpus(app, output, 'strict', timed_actions=False)
        self.source = self.corpus.source
        self.source.artifact_quota = ArtifactQuota() if scripted else ArtifactQuota(limit=128 << 20, max_files=16384)
        Gtk.Settings.get_default().set_property('gtk-cursor-blink', False)
        self.source.register('tooltip-root', 1, lambda: {'label': 'Controlled tooltip root'})
        self.source.register('tooltip-label', 6, lambda: {'label': 'Controlled native tooltip'}, parent='tooltip-root')
        self.tooltip = TooltipCapture(self.source, output)
        self.tooltip.set_parent(self.corpus.button)
        self.tooltip.parent_capture = self.corpus.capture
        self.source.register('modal-root', 1, lambda: {'label': 'Native transient modal'})
        self.modal_entry = Gtk.Entry(text='Modal scalar text: 𝄞 é שלום')
        self.source.register('modal-entry', 5, lambda: {'label': 'Modal entry', 'text': editable_text(self.modal_entry)}, parent='modal-root', actions=['Focus', 'ReplaceText', 'SetSelection'])
        self.modal_entry.connect('changed', lambda *_: self.source.changed())
        self.modal_entry.connect('notify::cursor-position', lambda *_: self.source.changed())
        self.modal_entry.connect('notify::selection-bound', lambda *_: self.source.changed())
        self.modal = Gtk.Window(application=app, title='Controlled native modal', transient_for=self.corpus.window, modal=True, decorated=False)
        self.modal.set_default_size(320, 110)
        self.modal_capture = CaptureWidget(self.source, self.modal_entry, 'modal', 'modal-root', output, role=1)
        self.modal.set_child(self.modal_capture)
        self.captures = {'main-content': self.corpus.capture, 'menu-popup': self.corpus.popup_capture, 'tooltip': self.tooltip, 'modal': self.modal_capture}
        self.windows = {'main-content': self.corpus.window, 'menu-popup': self.corpus.popup_capture, 'tooltip': self.tooltip, 'modal': self.modal}
        self.positions = {'main-content': [0, 0], 'menu-popup': [40, 330], 'tooltip': [300, 300], 'modal': [120, 40]}
        self.stack = {'main-content': 0, 'menu-popup': 1, 'tooltip': 2, 'modal': 3}
        self.revision = 0
        self.focus_generation = 1
        self.previous_focus = None
        self.publications = []
        self.interrupt = False
        self.interrupted_nonce = None
        self.on_tooltip_retired = None
        self.failures = []
        self.budget = CaptureBudget()
        self.view_id = self.source.identifier('whole-view')
        self.session_id = self.source.identifier('whole-view-session')
        # All source surface identities are allocated before reading a boundary.
        for key, capture in self.captures.items():
            self.source.identifier('surface:' + key)
            capture.capture_transaction = None
            capture.connect('map', lambda *_: self.source.changed())
            capture.connect('unmap', lambda *_, c=capture: c.publish_lifecycle())
        self.barrier = CaptureTransaction(self.source, self.oracle, self.request, self.commit, budget=self.budget, timeout=3, on_candidate=self.on_candidate)
        for key, capture in self.captures.items():
            self.barrier.register(key, capture)
            capture.capture_transaction = self.barrier
        self.corpus.menu.popup()
        self.tooltip.popup()
        self.modal.present()
        self.modal_entry.grab_focus()
        # The source owns the scenario clock. No animations, blinking caret,
        # arbitrary timer or external actor mutates visuals within a barrier.
        if scripted:
            GLib.timeout_add(400, self.start_initial)
        GLib.timeout_add(50, self.deadline_tick)
        if scripted:
            GLib.timeout_add(8000, self.watchdog)

    def oracle(self):
        surfaces = {}
        # This native re-read is serialized with all source callbacks. It also
        # computes stable Text/Semantic/Collection clocks at both boundaries.
        modal_live = self.modal_capture.source_alive and self.modal_capture.get_mapped()
        focus = None
        if modal_live and self.modal.get_focus() is not None:
            focus = ('modal', self.source.identifier('semantic:modal-entry'))
        else:
            # Resolve only explicitly bound native objects, including a popup
            # root when GTK focus is inside its source-owned menu descendants.
            for owner in sorted(self.corpus.registry.surfaces, key=lambda key: self.stack[key], reverse=True):
                capture = self.captures[owner]
                if capture.source_alive and capture.get_mapped():
                    key = self.corpus.registry.focus(owner)
                    if key:
                        focus = (owner, self.source.identifier('semantic:' + key))
                        break
        for key, capture in self.captures.items():
            alive, mapped = capture.source_alive, capture.source_alive and capture.get_mapped()
            semantics = self.source.semantics(capture.semantic_root, 1) if alive else []
            native_parent = (self.modal.get_transient_for() == self.corpus.window if key == 'modal' else
                self.tooltip.get_parent() == self.corpus.button if key == 'tooltip' else
                self.corpus.popup_capture.get_parent() == self.corpus.menu if key == 'menu-popup' else
                self.corpus.window.get_child() == capture)
            if alive and not native_parent:
                raise BarrierRejected('registered-native-parent-changed')
            surfaces[key] = {'alive': alive, 'mapped': mapped, 'native_parent_matches': native_parent,
                'surface_id': self.source._ids.get('surface:' + key),
                'semantic_root': self.source._ids.get('semantic:' + capture.semantic_root),
                'size': [capture.get_width(), capture.get_height()],
                'origin': self.positions[key], 'stacking_order': self.stack[key],
                'parent': None if key == 'main-content' else self.source._ids.get('surface:main-content'),
                'modal_for': self.source._ids.get('surface:main-content') if key == 'modal' and mapped else None,
                'native_modal': self.modal.get_modal() if key == 'modal' else False,
                'semantics': semantics}
        return {'incarnation': self.source.identity, 'source_revision': self.source.source_revision,
            'surfaces': surfaces, 'focus': focus,
            'visual_policy': {'animations': False, 'cursor_blink': False, 'controlled_static_canvas': True}}

    def request(self, nonce, required):
        def queue_fresh():
            # A restart can occur inside a GTK snapshot callback. Schedule its
            # redraw after that submission; queue_draw inside do_snapshot may
            # be consumed by the frame already being submitted.
            if self.barrier.active and self.barrier.nonce == nonce:
                for key in sorted(required):
                    self.captures[key].queue_draw()
            return False
        GLib.idle_add(queue_fresh)

    def start_initial(self):
        try:
            self.stage = 'all-mapped'
            self.barrier.begin()
        except Exception as error:
            self.failed(error)
        return False

    def on_candidate(self, nonce, key):
        if self.interrupt:
            self.interrupt = False
            self.interrupted_nonce = nonce
            # Deliberately mutate semantics AND destroy a registered transient
            # after a candidate was retained. Lifecycle invalidates the union.
            self.corpus.label.set_text('Barrier semantic mutation retained')
            self.source.changed()
            self.tooltip.retire_source()
            self.tooltip.popdown()
            self.tooltip.unparent()
            if self.on_tooltip_retired:
                self.on_tooltip_retired()

    def commit(self, nonce, facts, candidates, offers):
        self.revision += 1
        frame = self.assemble(nonce, facts, candidates, offers)
        if hasattr(self.source, 'private_generation'):
            frame['source_generation'] = self.source.private_generation
        if self.command_cause is not None:
            frame['command_cause'] = self.command_cause
        if not self.scripted:
            frame['barrier_events'] = list(self.barrier.events)
            frame['interrupted_nonce'] = self.interrupted_nonce
            frame['capture_budget'] = {'limit': self.budget.limit, 'peak': self.budget.peak}
            frame['fallback_probe'] = self.corpus.capture.export_probe()
        binding = {'session_id': '01' * 16, 'epoch': self.source.identity, 'view_id': f'{self.view_id:032x}'}
        envelope = encode_snapshot(frame, binding)
        frame.update(binding=binding, envelope_sha256=hashlib.sha256(envelope).hexdigest())
        frame['protocol_handoff'] = {'envelope_bytes': len(envelope), 'resource_bytes': sum(x['total_bytes'] for x in offers.values()),
            'resource_kind_counts': {str(kind): sum(x['kind'] == kind for x in offers.values()) for kind in (1, 2)}}
        label = f'whole-{self.revision:04d}'
        self.local_reference(label, frame)
        manifest = json.dumps(frame, sort_keys=True, indent=2) + '\n'
        if len(manifest.encode()) > MAX_MANIFEST_BYTES:
            raise BarrierRejected('whole-view-manifest-ceiling')
        self.source.write_artifact(self.output, label + '.bin', envelope)
        self.source.write_artifact(self.output, label + '.json', manifest.encode())
        if not self.scripted:
            if self.consumer is None:
                raise BarrierRejected('whole-source-consumer-missing')
            self.consumer(self, frame, label)
            return
        self.publications.append({'stage': self.stage, 'label': label, 'nonce': nonce, 'revision': self.revision,
            'surfaces': [s['surface_key'] for s in frame['surfaces']], 'mapped': sorted(candidates),
            'tokens': {key: value['capture_token'] for key, value in candidates.items()}, 'source_revision': facts['source_revision']})
        # Commit callback does not retain candidate frames/resources in memory.
        GLib.idle_add(self.next_stage)

    def local_reference(self, label, frame):
        # Diagnostic composition of each exact submitted native node's local
        # draw, under this source's declared isolated coordinates and stacking.
        # This is not a window-manager screenshot and no pixels enter RUI.
        width, height = frame['size']
        image = cairo.ImageSurface(cairo.FORMAT_ARGB32, width, height)
        context = cairo.Context(image)
        for surface in sorted(frame['surfaces'], key=lambda s: (s['stacking_order'], s['surface_id'])):
            if not surface['mapped']:
                continue
            reference = surface['local_reference']
            size = reference['stride'] * reference['height']
            if not 0 < size <= 4 << 20:
                raise BarrierRejected('local-reference-ceiling')
            path = self.output / (surface['local_reference_label'] + '.local.bgra')
            if path.stat().st_size != size:
                raise BarrierRejected('local-reference-length')
            pixels = bytearray(path.read_bytes())
            if hashlib.sha256(pixels).hexdigest() != reference['sha256']:
                raise BarrierRejected('local-reference-hash')
            source = cairo.ImageSurface.create_for_data(pixels, cairo.FORMAT_ARGB32,
                reference['width'], reference['height'], reference['stride'])
            context.save()
            context.rectangle(*surface['origin'], *surface['size'])
            context.clip()
            context.set_source_surface(source, *surface['origin'])
            context.paint()
            context.restore()
        image.flush()
        pixels = bytes(image.get_data())
        self.source.write_artifact(self.output, label + '.local.bgra', pixels)
        png = io.BytesIO(); image.write_to_png(png)
        self.source.write_artifact(self.output, label + '.local.png', png.getvalue())
        frame['local_reference'] = {'width': width, 'height': height, 'stride': image.get_stride(),
            'format': 'cairo-ARGB32-native-little-endian-BGRA-premultiplied-sRGB',
            'sha256': hashlib.sha256(pixels).hexdigest(),
            'scope': 'local-source-owned-composition-never-wire'}
        frame['surface_key'] = 'whole'

    def assemble(self, nonce, facts, candidates, offers):
        scene, semantics, surfaces, gaps, glyphs, bounds = [], [], [], [], [], []
        origins = {}
        counters = {'schema': 2, 'source_nodes_visited': 0, 'source_node_types': {}, 'node_decisions': {}, 'decline_reasons': {}, 'geometry_extractions': {}, 'vector_records_emitted': 0, 'vector_primitive_types': {}, 'vector_resource_bodies': len(offers), 'unsupported_subtrees': 0, 'local_reference_draws': 0}
        for key, state in facts['surfaces'].items():
            if not state['alive']:
                continue
            source = candidates.get(key)
            lifecycle_root = None
            if source is None:
                lifecycle = Exporter(self.source, key, self.revision)
                lifecycle_root = lifecycle.record('whole-unmapped', {'type': 'Group'})
                self.source.reconcile_scene(key, lifecycle.scene)
                scene.extend(lifecycle.scene)
                counters['vector_records_emitted'] += 1
                counters['vector_primitive_types']['Group'] = counters['vector_primitive_types'].get('Group', 0) + 1
            token = source['capture_token'] if source else f'{self.source.identity}:{key}:{facts["source_revision"]}:unmapped:{nonce}'
            sgaps = source['gaps'] if source else []
            surface = {'surface_key': key, 'surface_id': state['surface_id'], 'revision': self.revision,
                'surface_role': self.captures[key].role, 'size': state['size'], 'origin': state['origin'],
                'stacking_order': state['stacking_order'], 'mapped': state['mapped'], 'focused': facts['focus'] is not None and facts['focus'][0] == key,
                'modal_for': state['modal_for'], 'capture_token': token, 'gaps': sgaps,
                'scene_root': source['scene_root'] if source else lifecycle_root, 'semantic_root': state['semantic_root'],
                'capture_boundary': source['capture_boundary'] if source else 'source-lifecycle-state',
                'capture_barrier_nonce': nonce}
            if state['parent']:
                surface['parent_surface'] = {'surface_id': state['parent']}
            surfaces.append(surface)
            semantics.extend(state['semantics'])
            if source:
                scene.extend(source['scene'])
                glyphs.extend(source['source_glyph_evidence'])
                for gap in sgaps:
                    gaps.append({**gap, 'path': key + '/' + gap['path']})
                bounds.extend({**node, 'path': key + '/' + node['path']} for node in source['gsk_node_bounds'])
                for digest, entries in source['geometry_resource_origins'].items():
                    origins.setdefault(digest, []).extend({**item, 'node_path': key + '/' + item['node_path']} for item in entries)
                for name, value in source['exporter_counters'].items():
                    if name in ('schema', 'vector_resource_bodies'):
                        continue
                    if isinstance(value, dict):
                        counts = Counter(counters[name]); counts.update(value); counters[name] = dict(counts)
                    else:
                        counters[name] += value
                surface['local_reference'] = source['local_reference']
                surface['local_reference_label'] = f'{key}-{source["revision"]:04d}'
        focus = facts['focus']
        if focus != self.previous_focus:
            self.focus_generation += 1
            self.previous_focus = focus
        session = {'id': self.session_id, 'version': self.revision, 'canvas': [0, 0, 720, 600], 'focus': focus[1] if focus else None,
            'focus_surface': facts['surfaces'][focus[0]]['surface_id'] if focus else None, 'focus_generation': self.focus_generation}
        token = f'{self.source.identity}:whole:{facts["source_revision"]}:{nonce}'
        return {'revision': self.revision, 'source_revision': facts['source_revision'], 'capture_token': token,
            'capture_boundary': 'source-thread-all-registered-surfaces-fresh-snapshot-barrier',
            'gtk_version': [4, 22, 4], 'capture_barrier_nonce': nonce, 'whole_view_stage': self.stage, 'size': [720, 600],
            'surfaces': surfaces, 'scene': scene, 'semantics': semantics, 'session': session,
            'gaps': gaps, 'resources': sorted(offers), 'resource_offers': list(offers.values()),
            'uses_glyph_outline': bool(glyphs), 'source_glyph_evidence': glyphs,
            'geometry_resource_origins': origins, 'gsk_node_bounds': bounds, 'exporter_counters': counters,
            'source_boundary_facts': facts,
            'composition_scope': 'source-owned isolated placement/stacking; excludes decorations and native WM composition'}

    def next_stage(self):
        try:
            if self.stage == 'all-mapped':
                self.stage = 'interrupted-tooltip-destruction'
                self.interrupt = True
            elif self.stage == 'interrupted-tooltip-destruction':
                self.stage = 'menu-unmapped-modal-closed'
                self.corpus.menu.popdown()
                self.modal.set_visible(False)
                self.modal_capture.publish_lifecycle()
                self.corpus.entry.grab_focus()
            elif self.stage == 'menu-unmapped-modal-closed':
                self.stage = 'menu-remapped'
                self.corpus.menu.popup()
            elif self.stage == 'menu-remapped':
                self.stage = 'menu-destroyed'
                self.corpus.popup_capture.retire_source()
                self.corpus.menu.set_popover(None)
            else:
                self.finish()
                return False
            # Allow native mapping/layout/focus notifications before the source
            # actor opens its next transaction. Those do not supply candidates.
            GLib.timeout_add(120, self.begin_next)
        except Exception as error:
            self.failed(error)
        return False

    def begin_next(self):
        try:
            self.barrier.begin()
        except Exception as error:
            self.failed(error)
        return False

    def finish(self):
        if len(self.publications) != 5 or self.interrupted_nonce is None:
            raise BarrierRejected('missing-native-scenario-evidence')
        events = list(self.barrier.events)
        self.barrier.close()
        report = {'publications': self.publications, 'barrier_events': events, 'interrupted_nonce': self.interrupted_nonce,
            'capture_budget': {'limit': self.budget.limit, 'peak': self.budget.peak, 'after_close': self.budget.used},
            'gtk_version': [Gtk.get_major_version(), Gtk.get_minor_version(), Gtk.get_micro_version()],
            'scope': 'controlled cooperating four-surface source; source-owned composition, no WM observation',
            'animations': False, 'cursor_blink': False, 'failures': self.failures,
            'artifact_quota': {'limit': self.source.artifact_quota.limit, 'bytes': self.source.artifact_quota.used,
                'max_files': self.source.artifact_quota.max_files, 'files': len(self.source.artifact_quota.files)}}
        if self.budget.used:
            raise BarrierRejected('capture-reservation-leak')
        self.source.write_artifact(self.output, 'whole-report.json', (json.dumps(report, indent=2, sort_keys=True) + '\n').encode())
        print(json.dumps({'whole_view': 'PASS', 'publications': len(self.publications), 'peak': self.budget.peak}), flush=True)
        self.app.quit()

    def failed(self, error):
        self.failures.append(str(error))
        try:
            if not self.scripted:
                self.source.write_artifact(self.output, 'whole-failure.json', json.dumps({'reason': str(error), 'events': list(self.barrier.events), 'required': sorted(self.barrier.required), 'candidates': sorted(self.barrier.candidates)}, sort_keys=True).encode())
        except (OSError, ValueError):
            pass  # Diagnostic quota/write failure cannot defer cancellation.
        finally:
            self.barrier.close()
        print('ERROR:', error, file=sys.stderr, flush=True)
        self.app.quit()

    def deadline_tick(self):
        if self.barrier.closed:
            return False
        if self.barrier.expire():
            self.failed(BarrierRejected('whole-view-native-deadline'))
            return False
        return True

    def watchdog(self):
        self.failed(BarrierRejected('whole-view-native-watchdog'))
        return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    app = Gtk.Application(application_id='org.rui.WholeViewFixture')
    app.connect('activate', lambda app: setattr(app, 'whole', WholeView(app, args.output)))
    return app.run(None)


if __name__ == '__main__':
    raise SystemExit(main())
