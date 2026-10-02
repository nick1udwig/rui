#!/usr/bin/env python3
"""Ordinary GTK control corpus captured at its actual snapshot boundary."""
from pathlib import Path
import argparse
import json
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import gi
for name, version in [("Gtk", "4.0"), ("Gsk", "4.0"), ("Graphene", "1.0")]:
    gi.require_version(name, version)
from gi.repository import Gtk, Gsk, Gdk, Graphene, GLib, Gio

from adapters.gtk4.capture import CaptureWidget, CapturePopover
from adapters.sdk.producer import CoherentSource


STRICT_CSS = b"""
* { animation: none; transition: none; box-shadow: none; text-shadow: none;
    background-image: none; border-radius: 0; font-family: 'Liberation Sans';
    font-size: 14px; color: #111111; opacity: 1; outline-style: none;
    outline-width: 0; border-color: #444444; }
window, box, list, row, scrolledwindow, viewport, popover, contents, stack {
    background-color: #ffffff; }
button, entry, text, menubutton, modelbutton {
    background-color: #eeeeee; border: 1px solid #444444; padding: 4px; }
selection { background-color: #aaaaaa; color: #000000; }
"""


def rgba(value):
    result = Gdk.RGBA()
    result.parse(value)
    return result


def bounds(x, y, width, height):
    return Graphene.Rect().init(x, y, width, height)


class VectorCanvas(Gtk.Widget):
    def __init__(self):
        super().__init__()
        self.set_size_request(220, 100)
    def do_snapshot(self, snapshot):
        snapshot.append_color(rgba("#ffffff"), bounds(0, 0, self.get_width(), self.get_height()))
        builder = Gsk.PathBuilder.new()
        builder.move_to(12, 70)
        builder.line_to(60, 12)
        builder.cubic_to(90, 2, 120, 40, 172, 65)
        builder.line_to(12, 70)
        builder.close()
        path = builder.to_path()
        snapshot.append_fill(path, Gsk.FillRule.WINDING, rgba("#336699"))
        stroke = Gsk.Stroke.new(3)
        stroke.set_line_join(Gsk.LineJoin.ROUND)
        stroke.set_line_cap(Gsk.LineCap.ROUND)
        snapshot.append_stroke(path, stroke, rgba("#111111"))


class Corpus:
    def __init__(self, app, output, theme):
        self.app, self.output, self.theme = app, output, theme
        self.source = CoherentSource()
        self.actions = []
        self.captures = []
        self.window = Gtk.ApplicationWindow(application=app, title="RUI cooperating GTK corpus")
        self.window.set_default_size(520, 540)
        if theme == "strict":
            provider = Gtk.CssProvider()
            provider.load_from_data(STRICT_CSS)
            Gtk.StyleContext.add_provider_for_display(self.window.get_display(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        settings = Gtk.Settings.get_default()
        settings.set_property("gtk-enable-animations", False)
        settings.set_property("gtk-theme-name", "Adwaita")
        self.source.register("main", 1, lambda: {"label": "GTK corpus"})
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        box.set_margin_top(8); box.set_margin_bottom(8); box.set_margin_start(8); box.set_margin_end(8)
        self.label = Gtk.Label(label="Ordinary GTK controls — café Ω")
        box.append(self.label)
        self.source.register("heading", 6, lambda: {"label": self.label.get_text()}, parent="main")
        self.entry = Gtk.Entry(text="source text")
        box.append(self.entry)
        self.source.register("entry", 5, lambda: {"label": "Editable text", "value": self.entry.get_text()}, parent="main", actions=["ReplaceText", "SetSelection"])
        self.entry.connect("changed", lambda *_: self.source.changed())
        self.button = Gtk.Button(label="Activate source action")
        box.append(self.button)
        self.source.register("button", 3, lambda: {"label": self.button.get_label()}, parent="main", actions=["Activate"])
        self.button.connect("clicked", self.clicked)
        self.list = Gtk.ListBox()
        self.list.set_selection_mode(Gtk.SelectionMode.SINGLE)
        for key, name in [("row-alpha", "Alpha"), ("row-beta", "Beta"), ("row-gamma", "Gamma")]:
            row = Gtk.ListBoxRow()
            row.set_child(Gtk.Label(label=name))
            self.list.append(row)
            self.source.register(key, 8, lambda row=row, name=name: {"label": name, "value": "selected" if row.is_selected() else "unselected"}, parent="list", actions=["Activate"])
        self.source.register("list", 7, lambda: {"label": "Stable-ID list"}, parent="main", actions=["ScrollTo"])
        self.list.connect("row-selected", lambda *_: self.source.changed())
        scroll = Gtk.ScrolledWindow()
        scroll.set_min_content_height(110)
        scroll.set_child(self.list)
        box.append(scroll)
        self.canvas = VectorCanvas()
        box.append(self.canvas)
        self.source.register("canvas", 9, lambda: {"label": "Source-authored vector canvas"}, parent="main")
        action = Gio.SimpleAction.new("corpus-menu", None)
        action.connect("activate", lambda *_: self.record_action("menu-activate"))
        app.add_action(action)
        model = Gio.Menu()
        model.append("Menu source action", "app.corpus-menu")
        self.menu = Gtk.MenuButton(label="Ordinary GTK menu")
        if theme == "strict":
            self.menu.set_child(Gtk.Label(label="Ordinary GTK menu"))
        box.append(self.menu)
        self.source.register("menu-button", 3, lambda: {"label": "Ordinary GTK menu"}, parent="main", actions=["Activate"])
        self.source.register("menu-popup", 1, lambda: {"label": "Menu popup"})
        self.source.register("menu-item", 3, lambda: {"label": "Menu source action"}, parent="menu-popup", actions=["Activate"])
        self.popup_capture = CapturePopover(self.source, model, "menu-popup", "menu-popup", output, self.captured)
        if theme == "strict":
            self.popup_capture.set_has_arrow(False)
        self.menu.set_popover(self.popup_capture)
        self.capture = CaptureWidget(self.source, box, "main-content", "main", output, self.captured)
        self.popup_capture.parent_capture = self.capture
        self.window.set_child(self.capture)
        self.window.present()
        GLib.timeout_add(350, self.edit)
        GLib.timeout_add(650, self.select)
        GLib.timeout_add(950, self.open_menu)
        GLib.timeout_add(1300, self.activate_menu)
        GLib.timeout_add(1650, self.close_menu)
        GLib.timeout_add(1900, self.hide_canvas)
        GLib.timeout_add(2100, self.show_canvas)
        GLib.timeout_add(2300, self.resize)
        GLib.timeout_add(2900, self.finish)

    def captured(self, widget, frame):
        self.captures.append({"surface": widget.surface_key, "revision": frame["revision"], "source_revision": frame["source_revision"], "node_counts": frame["gsk_node_counts"], "gaps": frame["gaps"], "capture_duration_ns": frame["capture_duration_ns"]})

    def record_action(self, name):
        self.source.changed()
        self.actions.append({"name": name, "source_revision": self.source.source_revision, "evidence": "application-callback"})
        self.capture.queue_draw()

    def clicked(self, *_):
        self.label.set_text("Source action applied")
        self.record_action("button-activate")

    def edit(self):
        self.entry.set_text("edited source: café Ω")
        self.entry.select_region(0, 6)
        self.button.emit("clicked")
        self.record_action("replace-text")
        return False

    def select(self):
        self.list.select_row(self.list.get_row_at_index(1))
        self.record_action("select-beta")
        return False

    def open_menu(self):
        self.menu.popup()
        self.record_action("open-menu")
        return False

    def activate_menu(self):
        self.app.activate_action("corpus-menu", None)
        return False

    def close_menu(self):
        self.menu.popdown()
        self.record_action("close-menu")
        return False

    def hide_canvas(self):
        self.canvas.set_visible(False)
        self.record_action("hide-canvas")
        return False

    def show_canvas(self):
        self.canvas.set_visible(True)
        self.record_action("show-canvas")
        return False

    def resize(self):
        self.window.set_default_size(640, 580)
        self.record_action("resize-source")
        return False

    def finish(self):
        summary = {"theme": self.theme, "gtk_version": [Gtk.get_major_version(), Gtk.get_minor_version(), Gtk.get_micro_version()], "cairo_version": __import__("cairo").cairo_version_string(), "captures": self.captures, "actions": self.actions, "stable_semantic_ids": {key: self.source.identifier("semantic:" + key) for key in self.source._semantics}, "retired_scene_ids": sorted(self.source.retired_scene_ids), "retained_resource_bytes": sum(len(resource["body"]) for resource in self.source.resources.values()), "capture_scope": "cooperating main content and explicitly registered menu popup; excludes window decorations/compositor"}
        (self.output / "corpus-summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
        if not self.captures or not any(frame["surface"] == "menu-popup" for frame in self.captures):
            print("ERROR: missing actual main or popup snapshot", file=sys.stderr)
            self.app.quit()
            return False
        print(json.dumps({"theme": self.theme, "frames": len(self.captures), "gaps": sum(len(frame["gaps"]) for frame in self.captures), "surfaces": sorted({frame["surface"] for frame in self.captures})}), flush=True)
        self.app.quit()
        return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--theme", choices=["default", "strict"], default="default")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    app = Gtk.Application(application_id="org.rui.GtkCorpus")
    app.connect("activate", lambda app: setattr(app, "corpus", Corpus(app, args.output, args.theme)))
    return app.run(None)


if __name__ == "__main__":
    raise SystemExit(main())
