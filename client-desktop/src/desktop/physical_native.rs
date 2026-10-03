//! Narrow local native key-name boundary. It never uses symbols or text to
//! guess an evdev physical identity. It borrows GTK's existing native display
//! and opens no additional display or native connection.
use crate::physical_input::{evdev_from_gdk_wayland_keycode, evdev_from_xkb_name};
use gtk4::{gdk, prelude::*};
use rui_protocol_core::resources::MemoryBudget;

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn rui_physical_x11_name(
        display: *mut std::ffi::c_void,
        keycode: u32,
        name: *mut u8,
    ) -> std::ffi::c_int;
}

/// Query the actual local native physical position on GTK's owning thread.
/// X11 mapping is refreshed for each down, while the original down identity
/// must be retained until release by PhysicalKeyCapture. Only the supported
/// GTK Linux backends are accepted; other local mappings remain unavailable.
pub fn evdev_keycode(display: &gdk::Display, keycode: u32, budget: &MemoryBudget) -> Option<u32> {
    if !gtk4::is_initialized_main_thread() {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        match display.type_().name() {
            "GdkWaylandDisplay" => evdev_from_gdk_wayland_keycode(keycode),
            "GdkX11Display" => x11_key_name(display, keycode, budget).and_then(evdev_from_xkb_name),
            _ => None,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (display, keycode, budget);
        None
    }
}

/// Bounded local physical position metadata, useful for native mapping probes.
/// This function never reads translated key symbols or text.
pub fn x11_key_name(
    display: &gdk::Display,
    keycode: u32,
    budget: &MemoryBudget,
) -> Option<[u8; 4]> {
    if !gtk4::is_initialized_main_thread()
        || display.type_().name() != "GdkX11Display"
        || !(8..=255).contains(&keycode)
    {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        // Names-only XKB query: at most256 fixed4-byte key positions.
        // Conservative headroom survives the complete synchronous
        // query and all native allocation/free operations it invokes.
        let _memory = budget.reserve(8192).ok()?;
        let mut name = [0u8; 4];
        // SAFETY: display is a live borrowed GDK object on its GTK
        // thread. The C adapter validates backend, keycode and native
        // map pointers, writes exactly4 bytes, and retains no pointer.
        let valid =
            unsafe { rui_physical_x11_name(display.as_ptr().cast(), keycode, name.as_mut_ptr()) };
        (valid != 0).then_some(name)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (display, keycode, budget);
        None
    }
}
