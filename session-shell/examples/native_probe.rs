//! Reproducible private-fd/source probe; never connects to a desktop compositor.
#[path = "../tests/common/mod.rs"]
mod common;
use common::*;
use prost::Message;
use rui_client_desktop::{RenderLimits, RenderProfile, Renderer, Viewport};
use rui_protocol_core::{
    commands::CommandStatus,
    model::{CursorShape, Record, SurfaceRole},
    wire::proto as pb,
};
use rui_session_shell::backend::Receipt;
use sha2::{Digest, Sha256};
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: native_probe OUTPUT.pam")?;
    let mut f = Fixture::new();
    let mut second = window(SurfaceRole::Toplevel, 145.0, 15.0, 45.0, 90.0, 1);
    second.color.red = 0.5;
    second.color.blue = 0.0;
    let second = f.backend.topology_mut().open(second)?;
    let mut popup = window(SurfaceRole::Popup, 40.0, 65.0, 45.0, 35.0, 2);
    popup.parent = Some(f.main);
    popup.color.green = 0.5;
    popup.color.blue = 0.0;
    let popup = f.backend.topology_mut().open(popup)?;
    let mut tooltip = window(SurfaceRole::Tooltip, 120.0, 3.0, 50.0, 8.0, 3);
    tooltip.parent = Some(f.main);
    tooltip.color.red = 0.5;
    tooltip.color.green = 0.5;
    tooltip.color.blue = 0.5;
    let tooltip = f.backend.topology_mut().open(tooltip)?;
    assert!(f.backend.topology_mut().focus(tooltip).is_err());
    f.backend.topology_mut().set_cursor(CursorShape::Pointer)?;
    let raw = f.raw(f.main, vec![motion(30.0, 40.0), key(21, true)]);
    assert_eq!(f.submit(&raw, 0).status, CommandStatus::Dispatched);
    f.pump(0);
    let retry = f.submit(&raw, 0);
    assert_eq!(retry.status, CommandStatus::Dispatched);
    assert_eq!(f.backend.pending_frames(), 0);
    f.backend.poll(&mut f.broker, 480)?;
    assert_eq!(f.backend.topology().text(f.main), Some("zzzz"));
    let raw = f.raw(f.main, vec![key(21, false)]);
    assert_eq!(f.submit(&raw, 480).status, CommandStatus::Dispatched);
    f.pump(480);
    let mut modal = window(SurfaceRole::Popup, 70.0, 30.0, 50.0, 30.0, 4);
    modal.parent = Some(f.main);
    modal.modal_for = Some(f.main);
    let modal = f.backend.topology_mut().open(modal)?;
    f.backend.topology_mut().focus(modal)?;
    assert!(!f.backend.topology().modal_allows(f.main));
    f.backend.topology_mut().close(modal)?;
    assert_eq!(f.backend.topology().focused_surface(), Some(f.main));
    // Actual topology/configuration creates new source routes; placements are
    // absolute and content scale is explicit, independent of framebuffer size.
    let mut moved = window(SurfaceRole::Toplevel, 140.0, 15.0, 40.0, 70.0, 1);
    moved.transform.xx = 1.1;
    moved.transform.yy = 1.1;
    f.backend.topology_mut().configure(
        second,
        moved.desktop_rect,
        moved.content_size,
        moved.transform,
        1,
    )?;
    f.backend.topology_mut().close(popup)?;
    let mut reopened = window(SurfaceRole::Popup, 40.0, 65.0, 45.0, 35.0, 2);
    reopened.parent = Some(f.main);
    let reopened = f.backend.topology_mut().open(reopened)?;
    assert_ne!(reopened, popup);
    // Local handler revokes the common lease, verifies native neutralization,
    // and leaves common cleanup for the host's complete multi-family fanout.
    let raw = f.raw(f.main, vec![key(42, true)]);
    assert_eq!(f.submit(&raw, 480).status, CommandStatus::Dispatched);
    f.pump(480);
    f.backend.local_input(&mut f.broker, 481)?;
    assert!(f.backend.held().keys.is_empty());
    assert!(f.broker.current_lease(481).is_none());
    let cleanup = f.broker.take_cleanup().ok_or("missing common cleanup")?;
    assert_eq!(cleanup.keys.len(), 1); // test coordinator owns acknowledgement
    let source = f.backend.topology().snapshot()?;
    let renderer = Renderer::new_with_profile(
        RenderLimits::default(),
        f.budget.clone(),
        RenderProfile {
            cursor_vectors: true,
        },
    )?;
    let frame = renderer.render_view(source.state(), Viewport::new(200, 120), &())?;
    frame.write_pam(&mut std::fs::File::create(output)?)?;
    let mut pixels = Sha256::new();
    for pixel in frame.pixels() {
        pixels.update(pixel.rgba8());
    }
    let wire = pb::Snapshot {
        revision: source.state().revision,
        records: source.records().to_vec(),
        ..Default::default()
    }
    .encode_to_vec();
    let cursor = source
        .state()
        .records
        .values()
        .find_map(|r| {
            if let Record::Session(s) = r {
                s.cursor.as_ref()
            } else {
                None
            }
        })
        .ok_or("missing cursor")?;
    assert_eq!(cursor.position, point(40.0, 50.0));
    let profile = f.backend.profile();
    let applied = f
        .backend
        .traces()
        .filter(|t| t.receipt == Receipt::AppliedToSource)
        .count();
    let cancelled = f
        .backend
        .traces()
        .filter(|t| t.receipt == Receipt::Cancelled)
        .count();
    println!(
        "{{\"scope\":\"isolated-owned-fd-only-session\",\"authority_fixture\":\"trusted test readiness; no native mirror Presented claim\",\"local_input_origin\":\"trusted shell handler; native local-device capture pending\",\"libei\":\"{}\",\"libeis\":\"{}\",\"xkbcommon\":\"{}\",\"keymap_sha256\":\"{}\",\"host_layout\":\"{}\",\"client_layout_assumption\":\"us\",\"physical_evdev_code\":21,\"repeat_delay_ms\":{},\"repeat_interval_ms\":{},\"logical_region\":[0,0,200,120],\"native_grant\":[40,80,600,240],\"framebuffer\":[1000,700],\"scale\":2,\"source_revision\":{},\"source_records\":{},\"retired_ids\":{},\"native_frames_applied\":{},\"native_frames_cancelled\":{},\"source_snapshot_sha256\":\"{}\",\"render_rgba8_sha256\":\"{}\",\"accounted_bytes\":{},\"memory_limit_bytes\":{}}}",
        profile.libei,
        profile.libeis,
        profile.xkbcommon,
        hex(&profile.keymap_sha256),
        profile.host_layout,
        profile.repeat_delay_ms,
        profile.repeat_interval_ms,
        source.state().revision,
        source.state().records.len(),
        source.state().retired.len(),
        applied,
        cancelled,
        hex(&Sha256::digest(&wire)),
        hex(&pixels.finalize()),
        f.budget.used(),
        f.budget.limit()
    );
    Ok(())
}
