use rui_client_desktop::*;
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{ObjectKind, ObjectRef, Record, VisualCoverage};
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 3 && !(args.len() == 4 && args[3] == "--scene") {
        return Err("usage: render_capture CAPTURE.json OUTPUT.pam [--scene]".into());
    }
    let artifact = CaptureArtifact::read(&args[1]).map_err(|e| format!("capture metadata: {e}"))?;
    let mut receiver = artifact.receiver()?;
    artifact
        .apply_trusted_fixture(&mut receiver)
        .map_err(|e| format!("capture application: {e}"))?;
    artifact.verify_native_glyph_evidence(&receiver)?;
    let width = artifact.manifest()["size"][0]
        .as_u64()
        .ok_or("missing width")?
        .try_into()?;
    let height = artifact.manifest()["size"][1]
        .as_u64()
        .ok_or("missing height")?
        .try_into()?;
    let renderer = Renderer::new(RenderLimits::default(), receiver.memory_budget())?;
    let viewport = Viewport::new(width, height);
    let frame = if args.len() == 4 {
        let root = artifact.manifest()["scene_root"]
            .as_u64()
            .ok_or("missing scene root")?;
        let surface_id = artifact.manifest()["surface_id"]
            .as_u64()
            .ok_or("missing surface")?;
        let Some(Record::Surface(surface)) = receiver.state()?.records.get(&surface_id) else {
            return Err("missing selected surface".into());
        };
        if surface.visual_coverage != VisualCoverage::Complete
            || surface.scene_root
                != Some(ObjectRef {
                    kind: ObjectKind::Scene,
                    id: root,
                })
            || artifact.manifest()["gaps"]
                .as_array()
                .is_none_or(|g| !g.is_empty())
        {
            return Err("selected scene is incomplete".into());
        }
        renderer.render_scene(
            receiver.state()?,
            ObjectRef {
                kind: ObjectKind::Scene,
                id: root,
            },
            viewport,
            &receiver,
        )
    } else {
        renderer.render_view(receiver.state()?, viewport, &receiver)
    }
    .map_err(|e| format!("render: {e}"))?;
    frame.write_pam(&mut File::create(&args[2])?)?;
    eprintln!(
        "{} revision={} work={}",
        PROFILE, frame.revision, frame.work
    );
    Ok(())
}
