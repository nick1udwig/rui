//! An owned isolated RUI session. Native input can reach only its private EIS
//! endpoint, never the user's compositor or an unrestricted input backend.
use rui_protocol_core::{
    input::{map_granted_region, InputError},
    model::{Point, Rect},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShellError {
    #[error("only an explicitly authorized isolated whole session may receive native input")]
    Scope,
    #[error("invalid or unsupported isolated session geometry")]
    Geometry,
    #[error("native backend failed: {0}")]
    Native(&'static str),
    #[error("isolated session limit exceeded")]
    Limit,
    #[error(transparent)]
    Input(#[from] InputError),
    #[error(transparent)]
    Memory(#[from] rui_protocol_core::resources::MemoryError),
    #[error(transparent)]
    Model(#[from] rui_protocol_core::model::ModelError),
    #[error(transparent)]
    Ledger(#[from] rui_protocol_core::commands::LedgerError),
    #[error(transparent)]
    Session(#[from] rui_protocol_core::session::SessionError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectionScope {
    IsolatedWholeSession,
    ApplicationOnly,
}
#[derive(Clone, Copy, Debug)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct RegionGrant {
    logical: Rect,
    granted: Rect,
    framebuffer: Framebuffer,
    physical_scale: f64,
}
impl RegionGrant {
    pub fn new(
        scope: InjectionScope,
        logical: Rect,
        granted: Rect,
        framebuffer: Framebuffer,
        physical_scale: f64,
    ) -> Result<Self, ShellError> {
        if scope != InjectionScope::IsolatedWholeSession {
            return Err(ShellError::Scope);
        }
        let finite = |r: Rect| {
            [r.x, r.y, r.width, r.height, r.x + r.width, r.y + r.height]
                .iter()
                .all(|n| n.is_finite() && n.abs() <= 1e6)
                && r.width > 0.0
                && r.height > 0.0
        };
        if !finite(logical)
            || !finite(granted)
            || granted.x < 0.0
            || granted.y < 0.0
            || [granted.x, granted.y, granted.width, granted.height]
                .iter()
                .any(|n| n.fract() != 0.0)
            || framebuffer.width == 0
            || framebuffer.height == 0
            || framebuffer.width > 16384
            || framebuffer.height > 16384
            || !physical_scale.is_finite()
            || !(0.125..=8.0).contains(&physical_scale)
        {
            return Err(ShellError::Geometry);
        }
        Ok(Self {
            logical,
            granted,
            framebuffer,
            physical_scale,
        })
    }
    pub fn logical(&self) -> Rect {
        self.logical
    }
    pub fn granted(&self) -> Rect {
        self.granted
    }
    pub fn framebuffer(&self) -> Framebuffer {
        self.framebuffer
    }
    pub fn physical_scale(&self) -> f64 {
        self.physical_scale
    }
    pub fn map(&self, point: Point) -> Result<Point, ShellError> {
        Ok(map_granted_region(point, self.logical, self.granted)?)
    }
    pub fn unmap(&self, point: Point) -> Result<Point, ShellError> {
        Ok(map_granted_region(point, self.granted, self.logical)?)
    }
}
#[cfg(feature = "native-eis")]
pub mod actor;
#[cfg(feature = "native-eis")]
pub mod backend;
#[cfg(feature = "native-eis")]
pub mod native;
#[cfg(feature = "source-window")]
pub mod source_window;
pub mod topology;
