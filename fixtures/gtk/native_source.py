#!/usr/bin/env python3
"""Controlled native source for the admitted-command pipe integration fixture."""
import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk

from fixtures.gtk.corpus import Corpus
from adapters.gtk4.native import NativeDispatcher
from fixtures.gtk.fallback_probe import FallbackProbe


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--fixture-hooks', action='store_true')
    args = parser.parse_args()
    app = Gtk.Application(application_id='org.rui.GtkAdmittedSource')
    with FallbackProbe() as probe:
        def activate(app):
            app.corpus = Corpus(app, args.output, 'strict', timed_actions=False)
            app.corpus.capture.export_probe = probe.report
            app.corpus.popup_capture.export_probe = probe.report
            app.dispatcher = NativeDispatcher(app.corpus, fixture_hooks=args.fixture_hooks)
        app.connect('activate', activate)
        return app.run(None)


if __name__ == '__main__':
    raise SystemExit(main())
