use rui_client_desktop::physical_input::*;
use rui_protocol_core::resources::MemoryBudget;

fn owner() -> PhysicalKeyOwner {
    PhysicalKeyOwner {
        connection_generation: 4,
        grant_serial: 7,
        surface: 11,
        routing_token: [9; 16],
    }
}

#[test]
fn physical_position_mapping_never_uses_translated_layout_symbols() {
    assert_eq!(evdev_from_xkb_name(*b"AD06"), Some(21)); // German host interprets physical Y as Z.
    assert_eq!(evdev_from_xkb_name(*b"AB01"), Some(44));
    assert_eq!(evdev_from_xkb_name(*b"LFSH"), Some(42));
    assert_eq!(evdev_from_xkb_name(*b"ESC\0"), Some(1));
    assert_eq!(evdev_from_xkb_name(*b"UP  "), Some(103));
    for name in [
        *b"AC00",
        *b"AD13",
        *b"z\0\0\0",
        *b"ESCx",
        *b"I999",
        *b"\0\0\0\0",
    ] {
        assert_eq!(evdev_from_xkb_name(name), None);
    }
    assert_eq!(evdev_from_gdk_wayland_keycode(29), Some(21));
    for keycode in [0, 8, 776, u32::MAX] {
        assert_eq!(evdev_from_gdk_wayland_keycode(keycode), None);
    }
}

#[test]
fn held_keys_suppress_repeat_and_keep_original_down_identity_and_owner() {
    let budget = MemoryBudget::new(8192);
    let mut capture = PhysicalKeyCapture::new(&budget).unwrap();
    assert_eq!(
        capture.press(owner(), 29, 21).unwrap(),
        Some(PhysicalKey {
            code: 21,
            pressed: true
        })
    );
    // Layout/mapping changes or repeated translated input never become a second down.
    assert_eq!(capture.press(owner(), 29, 44).unwrap(), None);
    let mut stale = owner();
    stale.grant_serial += 1;
    assert_eq!(
        capture.release(stale, 29),
        Err(PhysicalInputError::OwnerChanged)
    );
    assert_eq!(capture.held_len(), 1);
    assert_eq!(
        capture.release(owner(), 29).unwrap(),
        Some(PhysicalKey {
            code: 21,
            pressed: false
        })
    );
    assert_eq!(capture.release(owner(), 29).unwrap(), None);
    assert_eq!(capture.owner(), None);
    assert_eq!(
        capture.press(owner(), 29, 44).unwrap(),
        Some(PhysicalKey {
            code: 44,
            pressed: true
        })
    );
    assert_eq!(capture.clear(), Some(owner()));
    assert_eq!(capture.held_len(), 0);
    assert_eq!(budget.used(), 4096);
    drop(capture);
    assert_eq!(budget.used(), 0);
}

#[test]
fn limits_and_context_changes_preserve_the_cleanup_obligation() {
    assert!(PhysicalKeyCapture::new(&MemoryBudget::new(4095)).is_err());
    let mut capture = PhysicalKeyCapture::new(&MemoryBudget::new(8192)).unwrap();
    for code in 1..=MAX_PHYSICAL_KEYS as u32 {
        capture.press(owner(), code, code).unwrap();
    }
    assert_eq!(
        capture.press(owner(), 200, 200),
        Err(PhysicalInputError::Limit)
    );
    assert_eq!(capture.held_len(), MAX_PHYSICAL_KEYS);
    for stale in [
        PhysicalKeyOwner {
            surface: 12,
            ..owner()
        },
        PhysicalKeyOwner {
            connection_generation: 5,
            ..owner()
        },
        PhysicalKeyOwner {
            routing_token: [10; 16],
            ..owner()
        },
    ] {
        assert_eq!(
            capture.press(stale, 201, 201),
            Err(PhysicalInputError::OwnerChanged)
        );
        assert_eq!(capture.owner(), Some(owner()));
    }
    assert_eq!(capture.clear(), Some(owner()));
    assert_eq!(
        capture.press(owner(), 3, 0),
        Err(PhysicalInputError::KeyMapping)
    );
}
