/* Read the local physical XKB key name, never its translated symbol or text.
 * GTK owns display and its lifetime; this function runs on the GTK thread.
 * Only key names are requested, bounded by X11's 8-bit keycode range. */
#include <gdk/x11/gdkx.h>
#include <X11/XKBlib.h>
#include <stdint.h>
#include <string.h>

int rui_physical_x11_name(void *display, uint32_t keycode, unsigned char out[4]) {
    if (!display || !out || !GDK_IS_X11_DISPLAY(display) ||
        keycode < 8 || keycode > 255) {
        return 0;
    }
    /* GTK 4.18 deprecates the X11 backend as a whole. This deliberately
     * supports its pinned existing implementation, not a replacement API. */
    G_GNUC_BEGIN_IGNORE_DEPRECATIONS
    Display *connection = gdk_x11_display_get_xdisplay(GDK_DISPLAY(display));
    G_GNUC_END_IGNORE_DEPRECATIONS
    if (!connection) {
        return 0;
    }
    XkbDescPtr map = XkbGetMap(connection, 0, XkbUseCoreKbd);
    if (!map) {
        return 0;
    }
    /* The names reply supplies the keycode bounds too. A flags0 map may not
     * contain them until this names-only query has completed. */
    int valid = XkbGetNames(connection, XkbKeyNamesMask, map) == Success &&
        keycode >= map->min_key_code && keycode <= map->max_key_code &&
        map->names && map->names->keys;
    if (valid) {
        memcpy(out, map->names->keys[keycode].name, 4);
    }
    XkbFreeKeyboard(map, XkbAllComponentsMask, True);
    return valid;
}
