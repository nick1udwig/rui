"""Shared bounded assembly of registered coherent GTK surface captures."""
from collections import Counter
import hashlib
import io
import cairo
from adapters.gtk4.capture import Exporter
from adapters.gtk4.transaction import BarrierRejected


class CoherentView:
    size = (720, 600)

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
        scene, semantics, surfaces, gaps, glyphs, bounds, bindings = [], [], [], [], [], [], []
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
                'binding_regions': state['binding_regions'],
                'capture_boundary': source['capture_boundary'] if source else 'source-lifecycle-state',
                'capture_barrier_nonce': nonce}
            if state['parent']:
                surface['parent_surface'] = {'surface_id': state['parent']}
            surfaces.append(surface)
            semantics.extend(state['semantics'])
            bindings.extend(self.source.bindings(state['surface_id'], surface['scene_root'], state['binding_regions'], usable=state['mapped'] and not sgaps))
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
                if 'local_reference' in source:
                    surface['local_reference'] = source['local_reference']
                    surface['local_reference_label'] = f'{key}-{source["revision"]:04d}'
        focus = facts['focus']
        session = {'id': self.session_id, 'version': self.revision, 'canvas': [0, 0, *self.size], 'focus': focus[1] if focus else None,
            'focus_surface': facts['surfaces'][focus[0]]['surface_id'] if focus else None, 'focus_generation': self.focus_generation}
        token = f'{self.source.identity}:whole:{facts["source_revision"]}:{nonce}'
        return {'revision': self.revision, 'source_revision': facts['source_revision'], 'capture_token': token,
            'capture_boundary': 'source-thread-all-registered-surfaces-fresh-snapshot-barrier', 'native_context_profile': 'gtk-native-focus-context.v1',
            'gtk_version': [4, 22, 4], 'capture_barrier_nonce': nonce, 'whole_view_stage': self.stage, 'size': list(self.size),
            'surfaces': surfaces, 'scene': scene, 'semantics': semantics, 'session': session, 'visual_bindings': bindings,
            'gaps': gaps, 'resources': sorted(offers), 'resource_offers': list(offers.values()),
            'uses_glyph_outline': bool(glyphs), 'source_glyph_evidence': glyphs,
            'geometry_resource_origins': origins, 'gsk_node_bounds': bounds, 'exporter_counters': counters,
            'source_boundary_facts': facts,
            'composition_scope': 'source-owned isolated placement/stacking; excludes decorations and native WM composition'}

