"""Typed state reads from explicitly registered native GTK source objects."""
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk
import math


class SourceRejected(ValueError):
    pass


def invalidate_native_context(source):
    # A swallowed GObject callback exception must never erase an ABA event.
    # This latch belongs to this source incarnation; only replacement recovers.
    source.native_context_unavailable = True


def require_native_context(source):
    if getattr(source, 'native_context_unavailable', False):
        raise SourceRejected('native-context-observation-unavailable')


class NativeAction:
    """Explicit source-owned Gio action in a registered menu surface."""
    def __init__(self, action, capture):
        self.action, self.capture = action, capture

    def get_mapped(self):
        return self.capture.source_alive and self.capture.get_mapped()

    def get_sensitive(self):
        return self.action.get_enabled()

    def activate(self):
        self.action.activate(None)


def editable_text(widget):
    """GtkEditable positions count Unicode characters, never UTF-8 bytes."""
    delegate = widget.get_delegate() or widget
    anchor = delegate.get_property('selection-bound')
    # The pinned GtkEntry getter returned the sorted selection end for a
    # backward selection; its actual GtkText delegate preserves the cursor.
    focus = delegate.get_position()
    return {'content': widget.get_text(), 'selection': [anchor, focus],
            'editable': widget.get_editable(), 'selectable': True, 'status': 1}


class NativeRegistry:
    def __init__(self, source, window):
        self.source, self.window = source, window
        self.widgets = {}
        self.surfaces = {}
        self.windows = {}
        source.native_bindings = self
        window.connect('notify::focus-widget', self.focus_changed)

    def bind(self, key, widget, surface):
        if key in self.widgets or key not in self.source._semantics:
            raise ValueError('native semantic object identity already bound or unregistered')
        if isinstance(widget, Gtk.Widget):
            self.source.identifier('binding:' + key)
        self.widgets[key] = (widget, surface)
        if isinstance(widget, NativeAction):
            widget.action.connect('notify::enabled', lambda *_: self.source.changed())
        else:
            widget.connect('notify::sensitive', lambda *_: self.source.changed())
            if isinstance(widget, Gtk.Entry):
                for signal in ('changed', 'notify::cursor-position', 'notify::selection-bound'):
                    widget.connect(signal, lambda *_, k=key, owner=surface: self.observe_text(k, owner))

    def observe_text(self, key, owner):
        if getattr(self.source, 'native_context_unavailable', False):
            return
        if key not in self.source._semantics or owner not in self.surfaces:
            return
        capture = self.surfaces[owner]
        if not capture.source_alive:
            return
        # Read every actual editable transition, rather than only the final
        # frame. An edit/selection away and back still invalidates IME context.
        budget = getattr(self.source, 'capture_budget', None)
        scratch = None
        try:
            scratch = budget.reserve(16 << 20) if budget else None
            self.source.semantics(capture.semantic_root, capture.frames)
        except Exception:
            # PyGObject may swallow callback exceptions. Losing even one
            # transition makes every old editing context unsafe to dispatch.
            invalidate_native_context(self.source)
        finally:
            if scratch:
                scratch.close()

    def reference(self, key):
        registered = self.widgets.get(key)
        return self.source._ids.get('binding:' + key) if registered and isinstance(registered[0], Gtk.Widget) else None

    def regions(self, owner):
        """Source-owned, axis-aligned widget regions in capture coordinates.

        A binding survives hide/unmap while its semantic object survives. Its
        region is unavailable then, or when disabled, clipped or transformed
        beyond this narrow source profile. Gio actions have no inferred region.
        """
        capture = self.surfaces.get(owner)
        if capture is None or not capture.source_alive:
            return []
        result = []
        for key, (widget, surface) in self.widgets.items():
            if surface != owner or key not in self.source._semantics or not isinstance(widget, Gtk.Widget):
                continue
            bounds = None
            if (isinstance(widget, (Gtk.Button, Gtk.MenuButton, Gtk.Entry, Gtk.ListBoxRow))
                    and widget.get_mapped() and widget.is_visible() and widget.is_sensitive()
                    and widget.get_can_target() and self.source._semantics[key][3]):
                # compute_bounds alone overapproximates a rotated hit region.
                if self.axis_aligned(widget, capture):
                    ok, rectangle = widget.compute_bounds(capture)
                    if ok:
                        bounds = [rectangle.origin.x, rectangle.origin.y, rectangle.size.width, rectangle.size.height]
                        clip = [0, 0, capture.get_width(), capture.get_height()]
                        ancestor = capture if widget == capture else widget.get_parent()
                        while ancestor is not None and ancestor != capture:
                            if not ancestor.get_mapped() or not ancestor.is_visible() or not ancestor.is_sensitive():
                                bounds = None
                                break
                            if ancestor.get_overflow() == Gtk.Overflow.HIDDEN or isinstance(ancestor, Gtk.ScrolledWindow):
                                if not self.axis_aligned(ancestor, capture):
                                    bounds = None
                                    break
                                clipped, region = ancestor.compute_bounds(capture)
                                if not clipped:
                                    bounds = None
                                    break
                                clip = self.intersection(clip, [region.origin.x, region.origin.y, region.size.width, region.size.height])
                            ancestor = ancestor.get_parent()
                        if ancestor != capture:
                            bounds = None
                        if bounds is not None:
                            bounds = self.intersection(bounds, clip)
                            if bounds[2] <= 0 or bounds[3] <= 0 or not all(math.isfinite(value) for value in bounds):
                                bounds = None
            result.append({'id': self.source._ids['binding:' + key],
                           'semantic_id': self.source._ids['semantic:' + key],
                           'interaction_region': bounds})
        return result

    @staticmethod
    def axis_aligned(widget, capture):
        ok, transform = widget.compute_transform(capture)
        affine = transform.to_2d() if ok else (False,)
        return (affine[0] and all(math.isfinite(value) for value in affine[1:])
                and affine[2] == 0 and affine[3] == 0 and affine[1] > 0 and affine[4] > 0)

    @staticmethod
    def intersection(a, b):
        x, y = max(a[0], b[0]), max(a[1], b[1])
        return [x, y, max(0, min(a[0] + a[2], b[0] + b[2]) - x), max(0, min(a[1] + a[3], b[1] + b[3]) - y)]

    def surface(self, key, capture, window=None):
        self.surfaces[key] = capture
        self.windows[key] = window or self.window
        if window is not None and window != self.window:
            window.connect('notify::focus-widget', self.focus_changed)
        capture.focus_reader = lambda: self.focus(key)

    def focus(self, surface):
        widget = self.windows[surface].get_focus()
        while widget is not None:
            for key, (registered, owner) in self.widgets.items():
                if widget == registered:
                    return key if owner == surface and key in self.source._semantics else None
            widget = widget.get_parent()
        return None

    def focus_changed(self, *_):
        if getattr(self.source, 'native_context_unavailable', False):
            return
        try:
            for surface, capture in self.surfaces.items():
                self.source.focus_changed(surface, self.focus(surface))
                capture.queue_draw()
        except Exception:
            invalidate_native_context(self.source)

    def retire(self, key):
        self.widgets.pop(key, None)
        self.source.unregister(key)
        self.focus_changed()

    def resolve_event(self, incarnation, surface_id, target_id, source_revision, routing_token):
        """Final source identity check, serialized with native dispatch later.

        This does not authenticate/admit a command or replace the core ledger.
        Source lifecycle and incarnation remain mandatory even on a trusted pipe.
        """
        if incarnation != self.source.identity:
            raise SourceRejected('retired-source-incarnation')
        for surface, capture in self.surfaces.items():
            if self.source._ids.get('surface:' + surface) != surface_id:
                continue
            if not capture.source_alive or not capture.source_mapped:
                raise SourceRejected('unmapped-or-retired-source-surface')
            if source_revision != self.source.source_revision or capture.last_frame['routing_token'] != routing_token:
                raise SourceRejected('stale-source-context')
            return self.resolve_semantic(incarnation, surface_id, target_id)
        raise SourceRejected('unknown-source-surface')

    def resolve_semantic(self, incarnation, surface_id, target_id):
        """Semantic identity check; spatial frame freshness is irrelevant.

        The dispatcher separately re-reads actual relevant native fields/clocks.
        Raw input keeps the stricter route/frame check in resolve_event.
        """
        require_native_context(self.source)
        if incarnation != self.source.identity:
            raise SourceRejected('retired-source-incarnation')
        for surface, capture in self.surfaces.items():
            if self.source._ids.get('surface:' + surface) != surface_id:
                continue
            if not capture.source_alive or not capture.source_mapped:
                raise SourceRejected('unmapped-or-retired-source-surface')
            for key, (widget, owner) in self.widgets.items():
                if owner == surface and key in self.source._semantics and self.source._ids.get('semantic:' + key) == target_id:
                    if not widget.get_mapped() or not (widget.get_sensitive() if isinstance(widget, NativeAction) else widget.is_sensitive()):
                        raise SourceRejected('disabled-or-unmapped-target')
                    return key, widget
            raise SourceRejected('retired-source-object')
        raise SourceRejected('unknown-source-surface')
