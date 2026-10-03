//! Explicit native-font gate for the large source; no GUI presentation claim.
use prost::Message;
use rui_client_desktop::{RenderLimits, Renderer, ResourceProvider, ResourceView, Viewport};
use rui_host_broker::*;
use rui_protocol_core::{resources::MemoryBudget, session::*, wire::proto as p};
struct Resources<'a>(&'a SourceCapture);
impl ResourceProvider for Resources<'_> {
    fn get(&self, digest: &[u8; 32]) -> Option<ResourceView<'_>> {
        self.0
            .resources()
            .find(|(offer, _)| &offer.digest == digest)
            .map(|(offer, bytes)| ResourceView { offer, bytes })
    }
}
#[test]
#[ignore = "native gate: requires pinned Pango/Cairo, PyGObject and bundled font"]
fn every_large_source_cell_is_visible_and_one_twenty_updates_preserve_resource_closure() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let font = NativeSourceFontProvider::new(
        "/usr/bin/python3".into(),
        root.join("fixtures/render/native_font.py"),
    );
    let mut host = ControlledHost::new_performance(HostConfig::default(), &font).unwrap();
    let config = SessionConfig::default();
    let mut profiles = config.supported_profiles;
    profiles.insert("glyph.outline-single.v1".into());
    let session = host
        .negotiate(
            VerifiedAuthorization {
                principal: "large-source".into(),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: false,
                    actions: Default::default(),
                },
            },
            ClientHello {
                client_instance_id: [3; 16],
                supported_versions: [1].into_iter().collect(),
                supported_profiles: profiles,
                required_profiles: config.required_profiles,
                receiver_limits: ReceiverLimits::default(),
                resume: None,
            },
            0,
        )
        .unwrap();
    let snapshot = host.checkpoint(&session.client, session.view_id).unwrap();
    let envelope = p::Envelope::decode(snapshot.as_ref()).unwrap();
    let snapshot = p::Snapshot::decode(envelope.payload.as_slice()).unwrap();
    assert_eq!(snapshot.records.len(), PERFORMANCE_RECORDS);
    assert_eq!(snapshot.required_resources.len(), 1);
    host.acknowledge_applied(&session.client, session.view_id, 1)
        .unwrap();
    let render_budget = MemoryBudget::new(256 << 20);
    let renderer = Renderer::new(RenderLimits::default(), render_budget.clone()).unwrap();
    for count in [1, 20] {
        let before = host.diagnostics().source_commit;
        assert!(host.advance_performance(21, 0).is_err());
        assert_eq!(host.diagnostics().source_commit, before);
        assert!(host
            .take_outbound(&session.client, session.view_id)
            .unwrap()
            .is_none());
        host.advance_performance(count, 0).unwrap();
        let bytes = host
            .take_outbound(&session.client, session.view_id)
            .unwrap()
            .unwrap();
        let envelope = p::Envelope::decode(bytes.as_ref()).unwrap();
        let transaction = p::Transaction::decode(envelope.payload.as_slice()).unwrap();
        assert_eq!(transaction.mutations.len(), count);
        assert_eq!(transaction.required_resources, snapshot.required_resources);
        let mut surfaces = 0;
        let mut glyphs = 0;
        for mutation in transaction.mutations {
            match mutation.mutation.unwrap() {
                p::mutation::Mutation::Put(p::Record {
                    record: Some(p::record::Record::Surface(_)),
                }) => surfaces += 1,
                p::mutation::Mutation::Put(p::Record {
                    record: Some(p::record::Record::Scene(node)),
                }) => {
                    assert!(matches!(
                        node.primitive,
                        Some(p::scene_node::Primitive::GlyphRun(_))
                    ));
                    glyphs += 1;
                }
                _ => panic!("unexpected large-fixture mutation"),
            }
        }
        assert_eq!(surfaces, 1);
        assert_eq!(glyphs, count - 1);
        host.acknowledge_applied(&session.client, session.view_id, before + 1)
            .unwrap();
    }
    let source = host
        .source_capture(&session.client, session.view_id)
        .unwrap();
    let frame = renderer
        .render_view(
            source.state(),
            Viewport::new(PERFORMANCE_WIDTH as u32, PERFORMANCE_HEIGHT as u32),
            &Resources(&source),
        )
        .unwrap();
    for y in 0..50 {
        for x in 0..50 {
            assert!(
                (y * 12..y * 12 + 12).any(|py| (x * 20..x * 20 + 20).any(|px| frame
                    .pixel(px, py)
                    .unwrap()
                    .alpha
                    > 0.0)),
                "empty cell {x},{y}"
            );
        }
    }
    assert!(render_budget.peak() <= render_budget.limit());
    drop(frame);
    drop(renderer);
    assert_eq!(render_budget.used(), 0);
}
