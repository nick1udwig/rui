//! Local physical-key capture. Symbols, translated text and client repeat are
//! never forwarded under HOST_PHYSICAL ownership. Remote authority remains in
//! the connection worker; clearing this local state does not confirm release.
use rui_protocol_core::resources::{MemoryBudget, MemoryReservation};
use thiserror::Error;

pub const MAX_PHYSICAL_KEYS: usize = 128;

/// Local grant serials prevent delayed callbacks from using a newer lease.
/// A source route remains stable across ordinary visual effects; changing
/// focus, topology or authority requires cancellation before further input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalKeyOwner {
    pub connection_generation: u64,
    pub grant_serial: u64,
    pub surface: u64,
    pub routing_token: [u8; 16],
}
impl PhysicalKeyOwner {
    pub fn valid(self) -> bool {
        self.grant_serial != 0 && self.surface != 0 && self.routing_token != [0; 16]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalKey {
    pub code: u32,
    pub pressed: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PhysicalInputError {
    #[error("physical input capture limit reached")]
    Limit,
    #[error("physical input owner changed; release or disconnect is required")]
    OwnerChanged,
    #[error("local physical key mapping is unsupported")]
    KeyMapping,
}

#[derive(Clone, Copy, Default)]
struct HeldKey {
    hardware: u32,
    evdev: u32,
}

pub struct PhysicalKeyCapture {
    held: [HeldKey; MAX_PHYSICAL_KEYS],
    len: usize,
    owner: Option<PhysicalKeyOwner>,
    _memory: MemoryReservation,
}
impl PhysicalKeyCapture {
    pub fn new(budget: &MemoryBudget) -> Result<Self, PhysicalInputError> {
        let memory = budget
            .reserve(4096)
            .map_err(|_| PhysicalInputError::Limit)?;
        Ok(Self {
            held: [HeldKey::default(); MAX_PHYSICAL_KEYS],
            len: 0,
            owner: None,
            _memory: memory,
        })
    }
    pub fn owner(&self) -> Option<PhysicalKeyOwner> {
        self.owner
    }
    pub fn held_len(&self) -> usize {
        self.len
    }
    pub fn press(
        &mut self,
        owner: PhysicalKeyOwner,
        hardware: u32,
        evdev: u32,
    ) -> Result<Option<PhysicalKey>, PhysicalInputError> {
        if !owner.valid() || self.owner.is_some_and(|old| old != owner) {
            return Err(PhysicalInputError::OwnerChanged);
        }
        if hardware == 0 || evdev == 0 || evdev > 767 {
            return Err(PhysicalInputError::KeyMapping);
        }
        if self.held[..self.len]
            .iter()
            .any(|key| key.hardware == hardware)
        {
            // Native GDK autorepeat is suppressed; the authoritative host owns
            // its layout, repeat timing and actual held-state effects.
            return Ok(None);
        }
        if self.len == MAX_PHYSICAL_KEYS
            || self.held[..self.len].iter().any(|key| key.evdev == evdev)
        {
            return Err(PhysicalInputError::Limit);
        }
        self.held[self.len] = HeldKey { hardware, evdev };
        self.len += 1;
        self.owner = Some(owner);
        Ok(Some(PhysicalKey {
            code: evdev,
            pressed: true,
        }))
    }
    pub fn release(
        &mut self,
        owner: PhysicalKeyOwner,
        hardware: u32,
    ) -> Result<Option<PhysicalKey>, PhysicalInputError> {
        if self.owner.is_some_and(|old| old != owner) {
            return Err(PhysicalInputError::OwnerChanged);
        }
        let Some(index) = self.held[..self.len]
            .iter()
            .position(|key| key.hardware == hardware)
        else {
            return Ok(None);
        };
        let evdev = self.held[index].evdev;
        self.len -= 1;
        self.held[index] = self.held[self.len];
        self.held[self.len] = HeldKey::default();
        if self.len == 0 {
            self.owner = None;
        }
        // Retain the original down's physical identity even if the local XKB
        // mapping changes before release. Never translate a release anew.
        Ok(Some(PhysicalKey {
            code: evdev,
            pressed: false,
        }))
    }
    /// Forget local capture only. The caller must separately request priority
    /// lease release/disconnect whenever this returns a previous held owner.
    pub fn clear(&mut self) -> Option<PhysicalKeyOwner> {
        let owner = self.owner.take();
        self.held[..self.len].fill(HeldKey::default());
        self.len = 0;
        owner
    }
}

/// Decode supported XKB physical position names from the actual X11 server.
/// This does not inspect key symbols, layout groups or translated characters.
/// Unknown vendor positions remain unsupported instead of guessing an offset.
pub fn evdev_from_xkb_name(name: [u8; 4]) -> Option<u32> {
    let group = |prefix: &[u8; 2], max: u32, first: u32| {
        if &name[..2] != prefix || !name[2].is_ascii_digit() || !name[3].is_ascii_digit() {
            return None;
        }
        let index = u32::from(name[2] - b'0') * 10 + u32::from(name[3] - b'0');
        (index >= 1 && index <= max).then_some(first + index - 1)
    };
    for value in [
        group(b"AE", 12, 2),
        group(b"AD", 12, 16),
        group(b"AC", 11, 30),
        group(b"AB", 10, 44),
    ] {
        if value.is_some() {
            return value;
        }
    }
    if let Some(code) = group(b"FK", 10, 59) {
        return Some(code);
    }
    let length = name.iter().position(|c| *c == 0 || *c == b' ').unwrap_or(4);
    if name[length..].iter().any(|c| *c != 0 && *c != b' ') {
        return None;
    }
    match &name[..length] {
        b"ESC" => Some(1),
        b"BKSP" => Some(14),
        b"TAB" => Some(15),
        b"RTRN" => Some(28),
        b"LCTL" => Some(29),
        b"TLDE" => Some(41),
        b"LFSH" => Some(42),
        b"BKSL" => Some(43),
        b"RTSH" => Some(54),
        b"KPMU" => Some(55),
        b"LALT" => Some(56),
        b"SPCE" => Some(57),
        b"CAPS" => Some(58),
        b"NMLK" => Some(69),
        b"SCLK" => Some(70),
        b"KP7" => Some(71),
        b"KP8" => Some(72),
        b"KP9" => Some(73),
        b"KPSU" => Some(74),
        b"KP4" => Some(75),
        b"KP5" => Some(76),
        b"KP6" => Some(77),
        b"KPAD" => Some(78),
        b"KP1" => Some(79),
        b"KP2" => Some(80),
        b"KP3" => Some(81),
        b"KP0" => Some(82),
        b"KPDL" => Some(83),
        b"LSGT" => Some(86),
        b"FK11" => Some(87),
        b"FK12" => Some(88),
        b"KPEN" => Some(96),
        b"RCTL" => Some(97),
        b"KPDV" => Some(98),
        b"PRSC" => Some(99),
        b"RALT" => Some(100),
        b"HOME" => Some(102),
        b"UP" => Some(103),
        b"PGUP" => Some(104),
        b"LEFT" => Some(105),
        b"RGHT" => Some(106),
        b"END" => Some(107),
        b"DOWN" => Some(108),
        b"PGDN" => Some(109),
        b"INS" => Some(110),
        b"DELE" => Some(111),
        b"MUTE" => Some(113),
        b"VOL-" => Some(114),
        b"VOL+" => Some(115),
        b"POWR" => Some(116),
        b"KPEQ" => Some(117),
        b"PAUS" => Some(119),
        b"LWIN" => Some(125),
        b"RWIN" => Some(126),
        b"COMP" => Some(127),
        _ => None,
    }
}

/// GTK 4's Wayland backend passes the protocol evdev key as `key + 8` to
/// deliver_key_event, so EventControllerKey exposes XKB-style codes too.
/// Verified implementation: GNOME/gtk tag4.22.4,
/// gdk/wayland/gdkseat-wayland.c::keyboard_handle_key. Native acceptance uses
/// private X11; no native Wayland test is claimed by this conversion helper.
pub fn evdev_from_gdk_wayland_keycode(keycode: u32) -> Option<u32> {
    keycode
        .checked_sub(8)
        .filter(|code| *code > 0 && *code <= 767)
}
