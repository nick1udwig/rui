//! Render authoritative records/resources independently of publication replay.
use rui_client_desktop::{RenderLimits, Renderer, ResourceProvider, ResourceView, Viewport};
use rui_host_broker::{ControlledHost, HostConfig, NativeSourceFontProvider, SourceCapture};
use rui_protocol_core::{
    model::Record,
    model::VisualCoverage,
    resources::MemoryBudget,
    session::{ClientHello, Permissions, ReceiverLimits, SessionConfig, VerifiedAuthorization},
};
use std::{fs::File, path::PathBuf, sync::Arc};
struct SourceResources<'a>(&'a SourceCapture);
impl ResourceProvider for SourceResources<'_> {
    fn get(&self, digest: &[u8; 32]) -> Option<ResourceView<'_>> {
        self.0
            .resources()
            .find(|(offer, _)| &offer.digest == digest)
            .map(|(offer, bytes)| ResourceView { offer, bytes })
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let mut host = ControlledHost::new_corpus_with_fonts(
        HostConfig::default(),
        "private-source".into(),
        Arc::new(NativeSourceFontProvider::new(
            PathBuf::from("/usr/bin/python3"),
            root.join("fixtures/render/native_font.py"),
        )),
    )?;
    let mut profiles = SessionConfig::default().supported_profiles;
    profiles.insert("glyph.outline-single.v1".into());
    let session = host.negotiate(
        VerifiedAuthorization {
            principal: "source-reference".into(),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: false,
                actions: Default::default(),
            },
        },
        ClientHello {
            client_instance_id: [9; 16],
            supported_versions: [1].into_iter().collect(),
            supported_profiles: profiles,
            required_profiles: SessionConfig::default().required_profiles,
            receiver_limits: ReceiverLimits::default(),
            resume: None,
        },
        0,
    )?;
    let _publication = host.checkpoint(&session.client, session.view_id)?;
    let source = host.source_capture(&session.client, session.view_id)?;
    assert!(source
        .state()
        .records
        .values()
        .filter_map(|r| if let Record::Surface(s) = r {
            Some(s)
        } else {
            None
        })
        .all(|s| s.visual_coverage == VisualCoverage::Complete));
    let budget = MemoryBudget::new(128 << 20);
    let renderer = Renderer::new(RenderLimits::default(), budget.clone())?;
    let canvas = source
        .state()
        .records
        .values()
        .find_map(|r| {
            if let Record::Session(s) = r {
                Some(s.canvas)
            } else {
                None
            }
        })
        .ok_or("source has no Session canvas")?;
    let viewport = Viewport {
        width: canvas.width.ceil() as u32,
        height: canvas.height.ceil() as u32,
        origin: rui_protocol_core::model::Point {
            x: canvas.x,
            y: canvas.y,
        },
        scale: 1.0,
    };
    let frame = renderer.render_view(source.state(), viewport, &SourceResources(&source))?;
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: source_reference OUTPUT.pam")?;
    frame.write_pam(&mut File::create(output)?)?;
    println!("direct source: {} records, {} verified outlines, Complete {}x{} Session canvas within default work ceiling",source.state().records.len(),source.resources().count(),viewport.width,viewport.height);
    drop(frame);
    assert_eq!(budget.used(), 0);
    Ok(())
}
