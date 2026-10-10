"""Declared static opaque style for cooperating GTK content."""
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
