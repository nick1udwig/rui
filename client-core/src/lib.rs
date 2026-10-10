//! Portable session, interaction and edit queues shared by native and WASM clients.
pub mod command_queue;
pub mod composition;
pub mod interaction;
pub mod session;
pub mod source_controls;
pub mod typing;

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub origin: rui_protocol_core::model::Point,
    pub scale: f64,
}
impl Viewport {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            origin: rui_protocol_core::model::Point { x: 0.0, y: 0.0 },
            scale: 1.0,
        }
    }
}
pub mod wire_command;
