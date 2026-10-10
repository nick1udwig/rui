#!/usr/bin/env python3
"""Admitted native callbacks hand off one fresh whole-view capture transaction."""
import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk
from adapters.gtk4.native import NativeDispatcher
from fixtures.gtk.native_hooks import CorpusHooks
from adapters.gtk4.state import NativeAction
from fixtures.gtk.fallback_probe import FallbackProbe
from fixtures.gtk.whole_view import WholeView


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--fixture-hooks', action='store_true')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    app = Gtk.Application(application_id='org.rui.WholeAdmittedSource')
    with FallbackProbe() as probe:
        def activate(app):
            app.hold()
            whole = WholeView(app, args.output, scripted=False)
            corpus, source = whole.corpus, whole.source

            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
            whole.modal_entry.unparent()
            box.append(whole.modal_entry)
            close = Gtk.Button(label='Close native modal')
            box.append(close)
            box.set_parent(whole.modal_capture)
            whole.modal_capture.child = box
            source.register('modal-close', 3, lambda: {'label': close.get_label()}, parent='modal-root', actions=['Activate'])
            corpus.registry.bind('modal-close', close, 'modal')
            def close_modal(*_):
                whole.modal.set_visible(False)
                corpus.entry.grab_focus()
                source.changed()
            close.connect('clicked', close_modal)

            retire = Gtk.Button(label='Retire native tooltip')
            corpus.capture.child.append(retire)
            source.register('tooltip-retire', 3, lambda: {'label': retire.get_label(), 'states': {1: 1 if retire.get_sensitive() else 2}}, parent='main', actions=['Activate'])
            corpus.registry.bind('tooltip-retire', retire, 'main-content')
            def retire_control():
                retire.set_visible(False)
                corpus.registry.retire('tooltip-retire')
                source.changed()
            whole.on_tooltip_retired = retire_control
            def retire_tooltip(*_):
                whole.tooltip.retire_source()
                whole.tooltip.popdown()
                whole.tooltip.unparent()
                retire_control()
                source.changed()
            retire.connect('clicked', retire_tooltip)

            # The source explicitly binds the Gio action used by its own menu
            # model; no title, widget-label search or accessibility inference.
            action = app.lookup_action('corpus-menu')
            corpus.registry.bind('menu-item', NativeAction(action, corpus.popup_capture), 'menu-popup')
            def menu_action(*_):
                corpus.label.set_text('Native menu action applied')
                corpus.menu.popdown()
                source.changed()
            action.connect('activate', menu_action)
            for capture in whole.captures.values():
                capture.export_probe = probe.report
            dispatcher = NativeDispatcher(corpus.integration, fixture_hooks=CorpusHooks(corpus) if args.fixture_hooks else False, whole=whole)
            for widget, signal in [(whole.modal_entry, 'changed'), (close, 'clicked'),
                                   (retire, 'clicked'), (action, 'activate')]:
                widget.connect(signal, dispatcher.observed_callback)
            app.whole, app.dispatcher = whole, dispatcher
            app.connect('shutdown', lambda *_: dispatcher.close())
        app.connect('activate', activate)
        return app.run(None)


if __name__ == '__main__':
    raise SystemExit(main())
