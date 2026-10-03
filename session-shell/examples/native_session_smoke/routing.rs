//! Actual native mapping and authenticated wire negatives, separate from held cleanup.
use super::*;
use rui_protocol_core::model::{Primitive, State};

pub async fn source_grid(
    root: &Path,
    local: &watch::Receiver<LocalObservation>,
) -> Result<(), Error> {
    // Only the local source frontend viewport changes. Authoritative desktop,
    // Surface geometry, EIS grants and the two receiver views remain separate.
    let before = *local.borrow();
    native(
        root,
        SOURCE,
        "resize_by",
        &[(600 - before.width).to_string(), "100".into()],
    )?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let current = *local.borrow();
        if current.width == 600 && current.height >= before.height + 92 {
            break;
        }
        if Instant::now() >= deadline {
            return Err("actual source viewport resize deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let height = local.borrow().height;
    if height % 2 != 0 {
        native(root, SOURCE, "resize_by", &["0".into(), "1".into()])?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while local.borrow().height != height + 1 {
            if Instant::now() >= deadline {
                return Err("source viewport pixel parity deadline".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    Ok(())
}

pub async fn letterbox(
    root: &Path,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<(), Error> {
    let displayed = observation
        .borrow()
        .displayed
        .ok_or("mapping requires actual initial paint")?;
    let origin = displayed.canvas_origin;
    let result = Command::new("/usr/bin/python3")
        .arg(root.join("fixtures/session-shell/native_mapping.py"))
        .args([
            (origin.x + 100.).to_string(),
            (origin.y + 180.).to_string(),
            (origin.x + 380.).to_string(),
            (origin.y + 300.).to_string(),
        ])
        .output()?;
    if !result.status.success() {
        return Err("actual local middle-button pan failed".into());
    }
    wait(observation, |state| {
        state.displayed.is_some_and(|d| {
            (d.stamp.viewport.origin.x + 280. / 1.5).abs() < 0.0001
                && (d.stamp.viewport.origin.y + 120. / 1.5).abs() < 0.0001
        })
    })
    .await?;
    Ok(())
}
fn activity(state: &State, surface: u64) -> Result<f64, Error> {
    let Some(Record::Surface(surface)) = state.records.get(&surface) else {
        return Err("mapping source surface missing".into());
    };
    let Some(Record::Scene(root)) = surface.scene_root.and_then(|r| state.records.get(&r.id))
    else {
        return Err("mapping source scene missing".into());
    };
    for child in &root.children {
        if let Some(Record::Scene(scene)) = state.records.get(&child.id) {
            if let Primitive::SolidRect { color, .. } = scene.primitive {
                if color.green == 0.25 && color.alpha == 1. {
                    return Ok(color.red);
                }
            }
        }
    }
    Err("mapping source effect missing".into())
}
fn cursor(state: &State) -> Result<Point, Error> {
    state
        .records
        .values()
        .find_map(|r| match r {
            Record::Session(s) => s.cursor.as_ref().map(|c| c.position),
            _ => None,
        })
        .ok_or("source cursor missing".into())
}
// Choose source pixels which coincide with mirror pixels to within the
// existing 1e-4 source-unit oracle. GDK's local drag offset is float32; this
// bound allows that small offset quantization, never a half-pixel rounding.
fn common_axis(
    mirror_scale: f64,
    mirror_origin: f64,
    source_scale: f64,
    source_offset: f64,
    low: f64,
    high: f64,
) -> Result<f64, Error> {
    for pixel in 0..2000 {
        let logical = (f64::from(pixel) - source_offset) / source_scale;
        if logical >= low && logical <= high {
            let remote = (logical - mirror_origin) * mirror_scale;
            if (remote - remote.round()).abs() / mirror_scale < 0.00005 {
                return Ok(logical);
            }
        }
    }
    Err("actual mirror/source pixel grids have no shared click point in bound".into())
}
pub async fn mapping(
    port: &NativeProducerPort,
    host: &HostPort,
    root: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    local: &watch::Receiver<LocalObservation>,
    before: &Compared,
    report: Option<&Path>,
) -> Result<(), Error> {
    let main = before
        .capture
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Surface(s) if s.focused => Some(s.clone()),
            _ => None,
        })
        .ok_or("mapping source focus missing")?;
    let viewport = before.displayed.stamp.viewport;
    if main.transform != identity() {
        return Err("mapping fixture requires its authored identity surface transform".into());
    }
    let left = -viewport.origin.x * viewport.scale;
    let top = -viewport.origin.y * viewport.scale;
    let right = f64::from(viewport.width) - left - 200. * viewport.scale;
    let bottom = f64::from(viewport.height) - top - 120. * viewport.scale;
    if viewport.scale != 1.5
        || [left, top, right, bottom]
            .iter()
            .any(|margin| *margin <= 0.)
    {
        return Err("actual mirror has no positive letterbox margins at scale1.5".into());
    }
    let local_frame = *local.borrow();
    let scale = (f64::from(local_frame.width) / 200.).min(f64::from(local_frame.height) / 120.);
    let offset = Point {
        x: (f64::from(local_frame.width) - 200. * scale) / 2.,
        y: (f64::from(local_frame.height) - 120. * scale) / 2.,
    };
    let target = Point {
        x: common_axis(1.5, viewport.origin.x, scale, offset.x, 24., 80.)?,
        y: common_axis(1.5, viewport.origin.y, scale, offset.y, 40., 78.)?,
    };
    let expected = Point {
        x: target.x - main.desktop_rect.x,
        y: target.y - main.desktop_rect.y,
    };
    let value = activity(before.capture.state(), main.id)?;
    let d = diagnostics(host).await?;
    native(root, CONTROLLER, "click", &point(before, target))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let remote = loop {
        let frame = compare(port, &observations[0]).await?;
        if activity(frame.capture.state(), main.id)? == value + 1. / 16. {
            if !same_native_point(cursor(frame.capture.state())?, target) {
                return Err("remote inverse point diverged".into());
            }
            break frame;
        }
        if Instant::now() >= deadline {
            return Err("remote click did not produce exactly one source effect".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let after = diagnostics(host).await?;
    if after.held_buttons != 0 || after.native_frames_applied <= d.native_frames_applied {
        return Err("remote native press/release was not actually received".into());
    }
    let callback_before = local.borrow().callbacks[2];
    native(
        root,
        SOURCE,
        "click",
        &[
            (local_frame.origin.x + offset.x + target.x * scale).to_string(),
            (local_frame.origin.y + offset.y + target.y * scale).to_string(),
        ],
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if local.borrow().callbacks[2] > callback_before && diagnostics(host).await?.lease.is_none()
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err("actual direct source click callback/takeover missing".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let frames = pair(port, observations).await?;
    let direct = cursor(frames[0].capture.state())?;
    if !same_native_point(direct, target)
        || activity(frames[0].capture.state(), main.id)? != value + 2. / 16.
    {
        return Err("direct and remote click coordinates/effects diverged".into());
    }
    let remote_point = cursor(remote.capture.state())?;
    if !same_native_point(
        Point {
            x: remote_point.x - main.desktop_rect.x,
            y: remote_point.y - main.desktop_rect.y,
        },
        expected,
    ) || !same_native_point(
        Point {
            x: direct.x - main.desktop_rect.x,
            y: direct.y - main.desktop_rect.y,
        },
        expected,
    ) {
        return Err("source-local inverse mismatch".into());
    }
    if let Some(report) = report {
        let mut out = File::create(report)?;
        writeln!(out,"{{\"scope\":\"actual GTK local pan, physical remote TLS/EIS and direct GDK source click\",\"routing\":\"mapping\",\"source_file_fingerprints\":{{{}}},\"mirror_scale\":1.5,\"letterbox_margins\":[{left},{top},{right},{bottom}],\"source_viewport\":[{},{}],\"mapping_error_tolerance\":0.0001,\"surface_local_point\":[{},{}],\"remote_effect_delta\":1,\"direct_effect_delta\":1,\"same_source_coordinates\":true,\"native_frames_added\":{},\"controller_records_sha256\":\"{}\",\"observer_records_sha256\":\"{}\",\"controller_after_paint_rgba_sha256\":\"{}\",\"observer_after_paint_rgba_sha256\":\"{}\"}}",source_hashes(root)?,local_frame.width,local_frame.height,expected.x,expected.y,after.native_frames_applied-d.native_frames_applied,hex(&frames[0].records),hex(&frames[1].records),hex(&frames[0].pixels),hex(&frames[1].pixels))?;
    }
    Ok(())
}
