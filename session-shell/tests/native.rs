#![cfg(feature = "native-eis")]
mod common;
use common::*;
use prost::Message;
use rui_client_desktop::{RenderLimits, RenderProfile, Renderer, Viewport};
use rui_protocol_core::{
    commands::CommandStatus,
    model::{CursorShape, SurfaceRole},
    wire::proto as pb,
};
#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn native_delivery_changes_authoritative_records_and_reference_pixels() {
    let mut f = Fixture::new();
    let renderer = Renderer::new_with_profile(
        RenderLimits::default(),
        f.budget.clone(),
        RenderProfile {
            cursor_vectors: true,
        },
    )
    .unwrap();
    let before = f.backend.topology().snapshot().unwrap();
    let frame0 = renderer
        .render_view(before.state(), Viewport::new(200, 120), &())
        .unwrap();
    let raw = f.raw(
        f.main,
        vec![motion(30.0, 40.0), key(21, true), key(21, false)],
    );
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    assert_eq!(f.backend.topology().text(f.main), Some("z")); // actual de XKB; physical US-Y code
    assert!(f.backend.held().keys.is_empty());
    assert_eq!(f.backend.traces().count(), 3);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    assert_eq!(f.backend.pending_frames(), 0);
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
    let after = f.backend.topology().snapshot().unwrap();
    let frame1 = renderer
        .render_view(after.state(), Viewport::new(200, 120), &())
        .unwrap();
    assert_ne!(frame0.pixels(), frame1.pixels());
    assert_eq!(frame0.pixels()[25 * 200 + 40].rgba8(), [0, 137, 255, 255]);
    assert_eq!(frame1.pixels()[25 * 200 + 40].rgba8(), [99, 137, 240, 255]);
    assert_eq!(f.backend.profile().region, [40, 80, 600, 240]);
    assert_ne!(f.backend.profile().keymap_sha256, [0; 32]);
    assert_eq!(
        f.backend.topology().grant().map(point(40.0, 50.0)).unwrap(),
        point(160.0, 180.0)
    );
    assert!(f.budget.used() <= f.budget.limit());
}
#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn modal_outside_batch_and_native_receiver_generation_guards() {
    let mut f = Fixture::new();
    let raw = f.raw(f.main, vec![motion(20.0, 20.0), motion(1000.0, 20.0)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Rejected);
    assert_eq!(f.backend.pending_frames(), 0);
    let raw = f.raw(f.main, vec![key(21, true)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    let mut modal = window(SurfaceRole::Popup, 50.0, 30.0, 60.0, 40.0, 10);
    modal.parent = Some(f.main);
    modal.modal_for = Some(f.main);
    let id = f.backend.topology_mut().open(modal).unwrap();
    f.backend.topology_mut().focus(id).unwrap();
    f.pump(0);
    assert_eq!(f.backend.topology().text(f.main), Some(""));
    let blocked = f.raw(f.main, vec![motion(20.0, 20.0)]);
    assert_eq!(f.submit(&blocked, 0).status, CommandStatus::Rejected);
    f.backend.local_input(&mut f.broker, 1).unwrap();
    assert!(f.backend.held().keys.is_empty());
    assert_eq!(f.backend.pending_frames(), 0);
    assert!(f.broker.current_lease(1).is_none());
    f.backend
        .topology_mut()
        .set_cursor(CursorShape::Text)
        .unwrap();
    f.backend.topology().snapshot().unwrap();
}
#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn actual_button_scroll_touch_frames_and_host_repeat_cleanup() {
    let mut f = Fixture::new();
    let p = || Some(pb::Point { x: 20.0, y: 20.0 });
    let raw = f.raw(
        f.main,
        vec![
            pb::raw_event::Event::Button(pb::ButtonEvent {
                code: 1,
                pressed: true,
                position: p(),
            }),
            pb::raw_event::Event::Wheel(pb::WheelEvent {
                x: 0.0,
                y: 2.0,
                position: p(),
            }),
            pb::raw_event::Event::Button(pb::ButtonEvent {
                code: 1,
                pressed: false,
                position: p(),
            }),
            pb::raw_event::Event::TouchDown(pb::TouchEvent {
                id: 99,
                position: p(),
            }),
            pb::raw_event::Event::TouchMove(pb::TouchEvent {
                id: 99,
                position: Some(pb::Point { x: 21.0, y: 22.0 }),
            }),
            pb::raw_event::Event::TouchUp(pb::TouchEnd { id: 99 }),
            key(21, true),
        ],
    );
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
    f.backend.poll(&mut f.broker, 399).unwrap();
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
    f.backend.poll(&mut f.broker, 480).unwrap();
    assert_eq!(f.backend.topology().text(f.main), Some("zzzz"));
    let duplicate = f.raw(f.main, vec![key(21, true)]);
    assert_eq!(f.submit(&duplicate, 480).status, CommandStatus::Rejected);
    f.backend.local_input(&mut f.broker, 481).unwrap();
    assert!(f.backend.held().keys.is_empty());
    f.backend.poll(&mut f.broker, 800).unwrap();
    assert_eq!(f.backend.topology().text(f.main), Some("zzzz"));
}

#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn idle_takeover_preserves_other_family_cleanup_and_actual_releases_remove_broker_state() {
    let mut f = Fixture::new();
    f.broker
        .set_composition(&f.lease, Some([88; 16]), 0)
        .unwrap();
    f.backend.local_input(&mut f.broker, 0).unwrap();
    let cleanup = f.broker.take_cleanup().unwrap();
    assert_eq!(cleanup.composition, Some([88; 16]));
    assert!(cleanup.keys.is_empty());
    f.lease = f
        .broker
        .acquire_control(&f.session.client, f.session.view_id, [10; 16], 0)
        .unwrap();
    for i in 0..140u64 {
        let raw = f.raw(
            f.main,
            vec![
                pb::raw_event::Event::TouchDown(pb::TouchEvent {
                    id: i + 1,
                    position: Some(pb::Point { x: 20.0, y: 20.0 }),
                }),
                pb::raw_event::Event::TouchUp(pb::TouchEnd { id: i + 1 }),
            ],
        );
        assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
        f.pump(0);
    }
    f.backend.local_input(&mut f.broker, 0).unwrap();
    let cleanup = f.broker.take_cleanup().unwrap();
    assert!(cleanup.touches.is_empty());
    assert!(cleanup.keys.is_empty());
    assert!(cleanup.buttons.is_empty());
    assert!(f
        .broker
        .acquire_control(&f.session.client, f.session.view_id, [11; 16], 0)
        .is_ok());
}
#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn authoritative_cover_hit_test_and_captured_stream_cleanup() {
    let mut f = Fixture::new();
    let mut cover = window(SurfaceRole::Popup, 20.0, 20.0, 40.0, 40.0, 10);
    cover.parent = Some(f.main);
    f.backend.topology_mut().open(cover).unwrap();
    let raw = f.raw(
        f.main,
        vec![pb::raw_event::Event::Button(pb::ButtonEvent {
            code: 1,
            pressed: true,
            position: Some(pb::Point { x: 20.0, y: 20.0 }),
        })],
    );
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Rejected);
    assert_eq!(f.backend.pending_frames(), 0);
    let raw = f.raw(
        f.main,
        vec![
            pb::raw_event::Event::Button(pb::ButtonEvent {
                code: 1,
                pressed: true,
                position: Some(pb::Point { x: 90.0, y: 70.0 }),
            }),
            motion(125.0, 70.0),
            pb::raw_event::Event::Button(pb::ButtonEvent {
                code: 1,
                pressed: false,
                position: Some(pb::Point {
                    x: 10000.0,
                    y: 10000.0,
                }),
            }),
        ],
    );
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    assert!(f.backend.held().buttons.is_empty());
    let raw = f.raw(f.main, vec![key(42, true)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    assert!(f.backend.held().keys.contains(&42));
    let spec = window(SurfaceRole::Toplevel, 11.0, 10.0, 120.0, 90.0, 0);
    f.backend
        .topology_mut()
        .configure(
            f.main,
            spec.desktop_rect,
            spec.content_size,
            spec.transform,
            0,
        )
        .unwrap();
    f.backend.poll(&mut f.broker, 0).unwrap();
    assert!(f.backend.held().keys.is_empty());
    let raw = f.raw(f.main, vec![key(21, true), key(21, false)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
}
#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn no_native_send_without_current_presented_and_exact_lease_generation() {
    let mut f = Fixture::new();
    let raw = f.raw(f.main, vec![key(21, true)]);
    f.broker
        .mark_resources_missing(&f.session.client, f.session.view_id)
        .unwrap();
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Rejected);
    assert_eq!(f.backend.pending_frames(), 0);
    f.broker
        .acknowledge_applied(&f.session.client, f.session.view_id, f.revision, true)
        .unwrap();
    f.broker
        .acknowledge_presented(&f.session.client, f.session.view_id, f.revision)
        .unwrap();
    let mut invalid =
        pb::RawInputBatch::decode(f.raw(f.main, vec![key(21, true)]).as_slice()).unwrap();
    invalid.presented_revision += 1;
    invalid.header.as_mut().unwrap().observed_view_revision = invalid.presented_revision;
    assert_ne!(
        f.submit(&invalid.encode_to_vec(), 0).status,
        CommandStatus::Dispatched
    );
    assert_eq!(f.backend.pending_frames(), 0);
}

#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn source_limit_after_native_effect_poisons_dispatch_until_confirmed_cleanup() {
    let mut f = Fixture::new();
    f.backend
        .topology_mut()
        .source_activity(f.main, &"x".repeat(4096))
        .unwrap();
    let raw = f.raw(f.main, vec![key(21, true)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while f.backend.poll(&mut f.broker, 0).is_ok() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(f.backend.held().keys.contains(&21));
    let next = f.raw(f.main, vec![key(21, false)]);
    assert!(f
        .backend
        .submit_raw(&mut f.broker, &mut f.ledger, &f.session.client, &next, 0)
        .is_err());
    f.backend.local_input(&mut f.broker, 1).unwrap();
    assert!(f.backend.held().keys.is_empty());
    let cleanup = f.broker.take_cleanup().unwrap();
    assert_eq!(cleanup.keys.len(), 1);
    assert_eq!(cleanup.keys[0].code, 21);
}

#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn local_handler_confirms_key_button_touch_cleanup_before_next_grant() {
    let mut f = Fixture::new();
    let raw = f.raw(
        f.main,
        vec![
            key(42, true),
            pb::raw_event::Event::Button(pb::ButtonEvent {
                code: 1,
                pressed: true,
                position: Some(pb::Point { x: 20.0, y: 20.0 }),
            }),
            pb::raw_event::Event::TouchDown(pb::TouchEvent {
                id: 7,
                position: Some(pb::Point { x: 20.0, y: 20.0 }),
            }),
        ],
    );
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    assert_eq!(f.backend.held().keys.len(), 1);
    assert_eq!(f.backend.held().buttons.len(), 1);
    assert_eq!(f.backend.held().touches.len(), 1);
    f.backend.local_input(&mut f.broker, 1).unwrap();
    assert!(f.backend.held().keys.is_empty());
    assert!(f.backend.held().buttons.is_empty());
    assert!(f.backend.held().touches.is_empty());
    assert_eq!(f.backend.pending_frames(), 0);
    assert!(f
        .broker
        .acquire_control(&f.session.client, f.session.view_id, [22; 16], 1)
        .is_err());
    let cleanup = f.broker.take_cleanup().unwrap();
    assert_eq!(cleanup.keys.len(), 1);
    assert_eq!(cleanup.buttons, vec![1]);
    assert_eq!(cleanup.touches, vec![7]);
    f.lease = f
        .broker
        .acquire_control(&f.session.client, f.session.view_id, [23; 16], 1)
        .unwrap();
    let raw = f.raw(f.main, vec![key(21, true), key(21, false)]);
    assert_eq!(f.submit(&raw, 1).status, CommandStatus::Dispatched);
    f.pump(1);
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
}

#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn resource_readiness_loss_neutralizes_host_repeat_and_cannot_rearm_old_key() {
    let mut f = Fixture::new();
    let raw = f.raw(f.main, vec![key(21, true)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
    assert!(f.backend.held().keys.contains(&21));
    f.broker
        .mark_resources_missing(&f.session.client, f.session.view_id)
        .unwrap();
    f.backend.poll(&mut f.broker, 480).unwrap();
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
    assert!(f.backend.held().keys.is_empty());
    assert_eq!(f.backend.pending_frames(), 0);
    assert_eq!(f.backend.traces().last().unwrap().command_sequence, 0);
    assert_eq!(
        f.backend.traces().last().unwrap().receipt,
        rui_session_shell::backend::Receipt::Cancelled
    );
    // Readiness loss is not a revocation of another family's context. The
    // matching broker held key is removed only after actual native release.
    assert!(f.broker.current_lease(480).is_some());
    assert!(f.broker.take_cleanup().is_none());
    f.broker
        .acknowledge_applied(&f.session.client, f.session.view_id, f.revision, true)
        .unwrap();
    f.broker
        .acknowledge_presented(&f.session.client, f.session.view_id, f.revision)
        .unwrap();
    f.backend.poll(&mut f.broker, 800).unwrap();
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
    let raw = f.raw(f.main, vec![key(21, true), key(21, false)]);
    assert_eq!(f.submit(&raw, 800).status, CommandStatus::Dispatched);
    f.pump(800);
    assert_eq!(f.backend.topology().text(f.main), Some("zz"));
    f.backend.local_input(&mut f.broker, 801).unwrap();
    assert!(f.broker.take_cleanup().unwrap().keys.is_empty());
}

#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn owner_generation_cancels_already_sent_native_frames_before_receipt() {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    let mut f = Fixture::new();
    let generation = Arc::new(AtomicU64::new(1));
    f.backend.bind_generation(generation.clone()).unwrap();
    let raw = f.raw(f.main, vec![key(21, true)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    generation.store(2, Ordering::Release);
    f.pump(0);
    assert_eq!(f.backend.topology().text(f.main), Some(""));
    assert_eq!(
        f.backend.traces().last().unwrap().receipt,
        rui_session_shell::backend::Receipt::Cancelled
    );
    f.backend.invalidate_local_input(&mut f.broker, 1).unwrap();
    assert!(f.backend.cleanup_pending());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while f.backend.cleanup_pending() {
        f.backend.poll_nonblocking(&mut f.broker, 1).unwrap();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(f.backend.held().keys.is_empty());
    assert_eq!(f.backend.pending_frames(), 0);
    assert!(f.broker.current_lease(1).is_none());
    assert!(f.broker.take_cleanup().is_some());
}

#[test]
#[ignore = "explicit pinned libei/libeis/XKB native gate"]
fn incremental_cleanup_holds_common_handoff_until_actual_release_and_stop() {
    let mut f = Fixture::new();
    let raw = f.raw(f.main, vec![key(42, true)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    f.backend.invalidate_local_input(&mut f.broker, 1).unwrap();
    assert!(f.backend.cleanup_pending());
    assert!(f
        .broker
        .acquire_control(&f.session.client, f.session.view_id, [24; 16], 1)
        .is_err());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while f.backend.cleanup_pending() {
        f.backend.poll_nonblocking(&mut f.broker, 1).unwrap();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(f.backend.held().keys.is_empty());
    let cleanup = f.broker.take_cleanup().unwrap();
    assert_eq!(cleanup.keys.len(), 1);
    f.lease = f
        .broker
        .acquire_control(&f.session.client, f.session.view_id, [25; 16], 1)
        .unwrap();
    let raw = f.raw(f.main, vec![key(21, true), key(21, false)]);
    assert_eq!(f.submit(&raw, 1).status, CommandStatus::Dispatched);
    f.pump(1);
    assert_eq!(f.backend.topology().text(f.main), Some("z"));
}
