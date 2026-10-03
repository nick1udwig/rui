"""Typed state reads from explicitly registered native GTK source objects."""
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk


class SourceRejected(ValueError):
    pass


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
        window.connect('notify::focus-widget', self.focus_changed)

    def bind(self, key, widget, surface):
        if key in self.widgets or key not in self.source._semantics:
            raise ValueError('native semantic object identity already bound or unregistered')
        self.widgets[key] = (widget, surface)
        if isinstance(widget, NativeAction):
            widget.action.connect('notify::enabled', lambda *_: self.source.changed())
        else:
            widget.connect('notify::sensitive', lambda *_: self.source.changed())

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
        for surface, capture in self.surfaces.items():
            self.source.focus_changed(surface, self.focus(surface))
            capture.queue_draw()

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
        if incarnation != self.source.identity:
            raise SourceRejected('retired-source-incarnation')
        for surface, capture in self.surfaces.items():
            if self.source._ids.get('surface:' + surface) != surface_id:
                continue
            if not capture.source_alive or not capture.source_mapped:
                raise SourceRejected('unmapped-or-retired-source-surface')
            for key, (widget, owner) in self.widgets.items():
                if owner == surface and key in self.source._semantics and self.source._ids.get('semantic:' + key) == target_id:
                    if not widget.get_mapped() or not widget.get_sensitive():
                        raise SourceRejected('disabled-or-unmapped-target')
                    return key, widget
            raise SourceRejected('retired-source-object')
        raise SourceRejected('unknown-source-surface')
