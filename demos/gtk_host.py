#!/usr/bin/env python3
"""Visible cooperating GTK application for the manual RUI demo.

Only the broker's inherited private socket accepts remote commands. Application
callbacks own the state; both local edits and admitted client edits use them.
"""
import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, Gio

from adapters.gtk4.native import NativeDispatcher
from adapters.gtk4.state import NativeAction
from fixtures.gtk.fallback_probe import FallbackProbe
from fixtures.gtk.whole_view import WholeView


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    app = Gtk.Application(application_id='org.rui.VisualDemoHost', flags=Gio.ApplicationFlags.NON_UNIQUE)
    with FallbackProbe() as probe:
        def activate(app):
            # A blinking GTK caret introduces transient opacity nodes outside
            # the cooperating exporter. Keep this manual demo within its
            # published vector coverage, including during local text edits.
            settings = Gtk.Settings.get_default()
            settings.set_property("gtk-cursor-blink", False)
            settings.set_property("gtk-enable-animations", False)
            whole = WholeView(app, args.output, scripted=False)
            corpus, source = whole.corpus, whole.source
            corpus.window.set_title('RUI Host')
            whole.modal.set_visible(False)
            whole.tooltip.popdown()
            corpus.menu.popdown()
            corpus.label.set_text('Host state: 0 clicks')
            corpus.entry.set_text('Edit this text in either window')
            corpus.button.set_label('Increment host counter')
            clicks = 0

            def increment(*_):
                nonlocal clicks
                clicks += 1
                corpus.label.set_text(f'Host state: {clicks} clicks')
                corpus.record_action('demo-increment')

            corpus.button.connect('clicked', increment)
            action = app.lookup_action('corpus-menu')
            corpus.registry.bind('menu-item', NativeAction(action, corpus.popup_capture), 'menu-popup')

            def menu_action(*_):
                corpus.label.set_text(f'Host menu action applied ({clicks} clicks)')
                corpus.menu.popdown()
                corpus.record_action('demo-menu')

            action.connect('activate', menu_action)
            for capture in whole.captures.values():
                capture.export_probe = probe.report
            # Geometry changes must also dirty the source for local polling.
            last_size = None

            def observe_size(widget, _clock):
                nonlocal last_size
                size = (widget.get_width(), widget.get_height())
                if size != last_size:
                    last_size = size
                    source.changed()
                return True

            corpus.capture.add_tick_callback(observe_size)
            corpus.entry.grab_focus()
            dispatcher = NativeDispatcher(corpus.integration, whole=whole, watchdog_seconds=0)
            app.whole, app.dispatcher = whole, dispatcher
            corpus.window.connect('close-request', lambda *_: (app.quit(), False)[1])
            app.connect('shutdown', lambda *_: dispatcher.close())
        app.connect('activate', activate)
        return app.run(None)


if __name__ == '__main__':
    raise SystemExit(main())
