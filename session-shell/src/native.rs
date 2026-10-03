//! Private fd-only EIS/libei transport. No native fd or injection method is
//! exposed to network callers. The common admission layer owns each endpoint.
use crate::{RegionGrant, ShellError};
use rui_protocol_core::resources::{MemoryBudget, MemoryReservation};
use sha2::{Digest, Sha256};
use std::{
    ffi::{c_char, c_int, c_void, CString},
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NativeEvent {
    pub kind: u32,
    pub sequence: u32,
    pub code: u32,
    pub pressed: u32,
    pub time: u64,
    pub touch_id: u64,
    pub x: f64,
    pub y: f64,
}
unsafe extern "C" {
    fn rui_native_new(
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        scale: f64,
        layout: *const c_char,
    ) -> *mut c_void;
    fn rui_native_destroy(ctx: *mut c_void);
    fn rui_native_next(ctx: *mut c_void, event: *mut NativeEvent) -> c_int;
    fn rui_native_ready(ctx: *mut c_void) -> c_int;
    fn rui_native_region(ctx: *mut c_void, region: *mut u32) -> c_int;
    fn rui_native_start(ctx: *mut c_void, sequence: u32) -> c_int;
    fn rui_native_send(
        ctx: *mut c_void,
        kind: u32,
        code: u32,
        pressed: u32,
        touch_id: u64,
        x: f64,
        y: f64,
    ) -> c_int;
    fn rui_native_frame(ctx: *mut c_void) -> u64;
    fn rui_native_stop(ctx: *mut c_void);
    fn rui_native_key(
        ctx: *mut c_void,
        code: u32,
        pressed: c_int,
        update: c_int,
        text: *mut c_char,
        capacity: usize,
    ) -> c_int;
    fn rui_native_repeats(ctx: *mut c_void, code: u32) -> c_int;
    fn rui_native_has_key(ctx: *mut c_void, code: u32) -> c_int;
    fn rui_native_clear_keys(ctx: *mut c_void) -> c_int;
    fn rui_native_keymap(ctx: *mut c_void, length: *mut usize) -> *const u8;
}
pub(crate) struct Endpoint {
    context: NonNull<c_void>,
    // libei/libeis/XKB opaque state is accessed only on its owning thread.
    _thread: PhantomData<Rc<()>>,
    _memory: MemoryReservation,
}
impl Endpoint {
    pub fn new(
        grant: RegionGrant,
        layout: &str,
        budget: &MemoryBudget,
    ) -> Result<Self, ShellError> {
        if !["us", "de"].contains(&layout) {
            return Err(ShellError::Native("unsupported host layout"));
        }
        // Pinned, trusted native keymaps/contexts have a fixed conservative
        // allowance. Native allocator RSS is not an instrumented wire budget.
        let memory = budget.reserve(2 * 1024 * 1024)?;
        let layout = CString::new(layout).map_err(|_| ShellError::Native("invalid layout"))?;
        let r = grant.granted();
        // SAFETY: bounded integer region and a live NUL-terminated fixed layout;
        // the bridge owns every returned native object until destroy.
        let context = NonNull::new(unsafe {
            rui_native_new(
                r.x as u32,
                r.y as u32,
                r.width as u32,
                r.height as u32,
                grant.physical_scale(),
                layout.as_ptr(),
            )
        })
        .ok_or(ShellError::Native("fd/keymap setup"))?;
        Ok(Self {
            context,
            _thread: PhantomData,
            _memory: memory,
        })
    }
    pub fn next(&mut self) -> Result<Option<NativeEvent>, ShellError> {
        let mut event = NativeEvent::default();
        // SAFETY: context is owned and alive; output has the bridge's repr(C)
        // layout. The C function releases all native event references.
        match unsafe { rui_native_next(self.context.as_ptr(), &mut event) } {
            0 => Ok(None),
            1 => Ok(Some(event)),
            _ => Err(ShellError::Native("native connection or protocol")),
        }
    }
    pub fn ready(&mut self) -> bool {
        // SAFETY: uniquely owned live native context.
        unsafe { rui_native_ready(self.context.as_ptr()) != 0 }
    }
    pub fn region(&mut self) -> Result<[u32; 4], ShellError> {
        let mut region = [0; 4];
        // SAFETY: four initialized output elements are writable for this call.
        if unsafe { rui_native_region(self.context.as_ptr(), region.as_mut_ptr()) } != 0 {
            return Err(ShellError::Native("missing granted region"));
        }
        Ok(region)
    }
    pub fn start(&mut self, sequence: u32) -> Result<(), ShellError> {
        // SAFETY: bridge rejects unready devices and zero sequences.
        if unsafe { rui_native_start(self.context.as_ptr(), sequence) } != 0 {
            return Err(ShellError::Native("device not ready"));
        }
        Ok(())
    }
    pub fn send(&mut self, event: NativeEvent) -> Result<(), ShellError> {
        // SAFETY: internal caller validates event type, held transitions and
        // every coordinate against the explicit grant before this call.
        if unsafe {
            rui_native_send(
                self.context.as_ptr(),
                event.kind,
                event.code,
                event.pressed,
                event.touch_id,
                event.x,
                event.y,
            )
        } != 0
        {
            return Err(ShellError::Native("native event send"));
        }
        Ok(())
    }
    pub fn frame(&mut self) -> u64 {
        // SAFETY: admission starts emulation before any hardware frame.
        unsafe { rui_native_frame(self.context.as_ptr()) }
    }
    pub fn stop(&mut self) {
        // SAFETY: caller drains/neutralizes all held state before stopping.
        unsafe { rui_native_stop(self.context.as_ptr()) }
    }
    pub fn key(&mut self, code: u32, pressed: bool, update: bool) -> Result<String, ShellError> {
        let mut text = [0u8; 64];
        // SAFETY: fixed output capacity is passed exactly; code has been
        // preflighted against the native keymap before admission.
        let length = unsafe {
            rui_native_key(
                self.context.as_ptr(),
                code,
                c_int::from(pressed),
                c_int::from(update),
                text.as_mut_ptr().cast(),
                text.len(),
            )
        };
        if length < 0 || length as usize >= text.len() {
            return Err(ShellError::Native("native key output"));
        }
        let value = std::str::from_utf8(&text[..length as usize])
            .map_err(|_| ShellError::Native("native UTF-8"))?;
        Ok(value.to_owned())
    }
    pub fn repeats(&mut self, code: u32) -> bool {
        // SAFETY: querying the uniquely owned keymap with an evdev code.
        unsafe { rui_native_repeats(self.context.as_ptr(), code) != 0 }
    }
    pub fn has_key(&mut self, code: u32) -> bool {
        // SAFETY: querying the uniquely owned keymap with an evdev code.
        unsafe { rui_native_has_key(self.context.as_ptr(), code) != 0 }
    }
    pub fn clear_keys(&mut self) -> Result<(), ShellError> {
        // SAFETY: the bridge replaces the source-owned state under the same
        // live keymap and releases its previous state.
        if unsafe { rui_native_clear_keys(self.context.as_ptr()) } != 0 {
            return Err(ShellError::Native("key neutralization"));
        }
        Ok(())
    }
    pub fn keymap_digest(&mut self) -> Result<[u8; 32], ShellError> {
        let mut length = 0;
        // SAFETY: the bridge retains this keymap text until context destruction;
        // its bounded length includes the final NUL required by EIS keymaps.
        let pointer = unsafe { rui_native_keymap(self.context.as_ptr(), &mut length) };
        if pointer.is_null() || length == 0 || length > 128 * 1024 {
            return Err(ShellError::Native("keymap metadata"));
        }
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
        Ok(Sha256::digest(bytes).into())
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: this is the only owner, on the construction thread; destroy
        // releases touches, devices, contexts, keymaps and the owned fd.
        unsafe { rui_native_destroy(self.context.as_ptr()) }
    }
}
