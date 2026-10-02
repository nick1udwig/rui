use prost::Message;
use rui_host_broker::*;
use rui_protocol_core::{
    commands::{CommandStatus, Id},
    model::{Primitive, Record},
    receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver},
    resources::*,
    session::*,
    wire::proto as p,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
const SECRET: &str = "PRIVATE-SOURCE-SHAPER-SENTINEL";
struct PublicProvider {
    calls: AtomicUsize,
}
impl SourceFontProvider for PublicProvider {
    fn shape(&self, text: &str, size: f64) -> Result<SourceShapedText, SourceFontError> {
        assert!(!text.contains(SECRET));
        self.calls.fetch_add(1, Ordering::Relaxed);
        if text.contains('😀') {
            return Err(SourceFontError);
        }
        let mut body = b"RVP1".to_vec();
        body.extend_from_slice(&4u32.to_be_bytes());
        for (code, xy) in [(1, [0.0f64, 0.0]), (2, [0.5, 0.0]), (2, [0.0, 0.7])] {
            body.push(code);
            for x in xy {
                body.extend_from_slice(&x.to_bits().to_be_bytes());
            }
        }
        body.push(5);
        let digest = resource_digest(
            ResourceKind::GlyphOutline,
            1,
            ResourceProvenance::FontOutline,
            &body,
        );
        let outline = SourceOutline::new(
            ResourceOffer {
                kind: ResourceKind::GlyphOutline,
                version: 1,
                provenance: ResourceProvenance::FontOutline,
                total_bytes: body.len() as u64,
                digest,
            },
            body,
        )?;
        Ok(SourceShapedText {
            glyphs: text
                .chars()
                .enumerate()
                .map(|(i, _)| SourceGlyph {
                    resource: digest,
                    size,
                    position: [if i % 2 == 0 { 10.0 } else { 0.0 }, 12.0],
                })
                .collect(),
            resources: vec![outline],
        })
    }
}
fn auth(control: bool) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: if control { "controller" } else { "observer" }.into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: control,
            actions: ["Activate", "ReplaceText", "SetSelection", "Focus"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        },
    }
}
fn hello(id: u8, semantic: bool) -> ClientHello {
    let mut h = ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits::default(),
        resume: None,
    };
    h.supported_profiles
        .insert("glyph.outline-single.v1".into());
    if semantic {
        h.supported_profiles.insert("semantics.only.v1".into());
        h.required_profiles.insert("semantics.only.v1".into());
    }
    h
}
fn context(s: &NegotiatedSession) -> ResourceContext {
    ResourceContext {
        scope: ResourceScope {
            session_id: s.session_id,
            epoch: s.epoch,
            principal_id: s.client.client_instance_id,
        },
        view_id: s.view_id,
    }
}
fn receive(
    host: &ControlledHost,
    s: &NegotiatedSession,
    receiver: &mut ViewReceiver,
    id: Id,
    bytes: &[u8],
) {
    for (offer, _) in host
        .source_capture(&s.client, s.view_id)
        .unwrap()
        .resources()
    {
        receiver.authorize_resource(offer.digest).unwrap();
    }
    match receiver.receive_envelope(id, bytes).unwrap() {
        ReceiveOutcome::NeedResources { missing, .. } => {
            for digest in missing.iter() {
                let resource = host.resource(&s.client, s.view_id, *digest).unwrap();
                receiver.authorize_resource(*digest).unwrap();
                receiver
                    .receive_resource_offer(context(s), resource.offer().clone())
                    .unwrap();
                receiver
                    .resource_chunk(context(s), *digest, 0, resource.bytes())
                    .unwrap();
                receiver.complete_resource(context(s), *digest).unwrap();
            }
        }
        ReceiveOutcome::Applied { .. } => {}
        other => panic!("{other:?}"),
    }
}
fn open(
    host: &mut ControlledHost,
    id: u8,
    semantic: bool,
    control: bool,
) -> (NegotiatedSession, ViewReceiver) {
    let s = host
        .negotiate(auth(control), hello(id, semantic), 0)
        .unwrap();
    let mut receiver = ViewReceiver::new(
        context(&s),
        s.limits.clone(),
        ReceiverConfig {
            single_glyph_outlines: true,
            ..Default::default()
        },
    )
    .unwrap();
    let bytes = host.checkpoint(&s.client, s.view_id).unwrap();
    receive(host, &s, &mut receiver, [1; 16], &bytes);
    host.acknowledge_applied(&s.client, s.view_id, 1).unwrap();
    if semantic {
        host.acknowledge_semantic_ready(&s.client, s.view_id, 1)
            .unwrap();
    }
    (s, receiver)
}
fn edit(
    s: &NegotiatedSession,
    lease: &ControlLease,
    r: &ViewReceiver,
    seq: u64,
    target: u64,
    text: Option<&str>,
) -> Vec<u8> {
    let state = r.state().unwrap();
    let Record::Semantic(n) = &state.records[&target] else {
        panic!()
    };
    let (text_version, end) = n
        .text
        .map(|r| {
            let Record::Text(t) = &state.records[&r.id] else {
                panic!()
            };
            (
                Some(t.text_version),
                t.content.as_ref().unwrap().chars().count() as u64,
            )
        })
        .unwrap_or((None, 0));
    let focus = state.records.values().find_map(|r| match r {
        Record::Session(s) => Some(s.focus_generation),
        _ => None,
    });
    p::SemanticCommand {
        epoch: s.epoch.to_vec(),
        view_id: s.view_id.to_vec(),
        client_instance_id: s.client.client_instance_id.to_vec(),
        command_seq: seq,
        control_lease_id: lease.lease_id.to_vec(),
        control_generation: lease.generation,
        target_id: target,
        action_id: if text.is_some() {
            "ReplaceText"
        } else {
            "Activate"
        }
        .into(),
        action_schema_version: 1,
        observed_view_revision: state.revision,
        preconditions: Some(p::Preconditions {
            target_version: Some(n.version),
            text_version,
            focus_generation: focus,
            model_version: None,
        }),
        arguments: Some(text.map_or(
            p::semantic_command::Arguments::NoArguments(p::EmptyArguments {}),
            |t| {
                p::semantic_command::Arguments::ReplaceText(p::TextArguments {
                    start: 0,
                    end,
                    text: t.into(),
                })
            },
        )),
    }
    .encode_to_vec()
}
#[test]
fn ordered_closed_public_glyphs_are_stable_and_private_content_never_shapes() {
    let provider = Arc::new(PublicProvider {
        calls: AtomicUsize::new(0),
    });
    let mut host = ControlledHost::new_corpus_with_fonts(
        HostConfig::default(),
        SECRET.into(),
        provider.clone(),
    )
    .unwrap();
    let initial_calls = provider.calls.load(Ordering::Relaxed);
    let (observer, replica) = open(&mut host, 4, false, false);
    assert_eq!(
        host.view_description(&observer.client, observer.view_id)
            .unwrap()
            .coverage,
        "complete"
    );
    let capture = host
        .source_capture(&observer.client, observer.view_id)
        .unwrap();
    assert_eq!(capture.resources().count(), 1);
    assert_eq!(capture.state().records, replica.state().unwrap().records);
    assert!(!host
        .snapshot(&observer.client, observer.view_id)
        .unwrap()
        .windows(SECRET.len())
        .any(|s| s == SECRET.as_bytes()));
    let Record::Scene(first) = &capture.state().records[&100128] else {
        panic!()
    };
    let Record::Scene(second) = &capture.state().records[&100129] else {
        panic!()
    };
    let Primitive::GlyphRun { positions: a, .. } = &first.primitive else {
        panic!()
    };
    let Primitive::GlyphRun { positions: b, .. } = &second.primitive else {
        panic!()
    };
    assert!(a[0].x > b[0].x); // source overlap order was not sorted by x/digest
    assert_eq!(provider.calls.load(Ordering::Relaxed), initial_calls);
    let digest = capture.resources().next().unwrap().0.digest;
    let mut wrong = observer.client.clone();
    wrong.principal = "mallory".into();
    assert!(host.resource(&wrong, observer.view_id, digest).is_err());
    let held = host
        .resource(&observer.client, observer.view_id, digest)
        .unwrap();
    host.disconnect(&observer.client, 1).unwrap();
    assert!(host
        .resource(&observer.client, observer.view_id, digest)
        .is_err());
    assert!(!held.bytes().is_empty());
}
#[test]
fn unsupported_unicode_edit_applies_semantics_and_publishes_truthful_partial_coverage() {
    let provider = Arc::new(PublicProvider {
        calls: AtomicUsize::new(0),
    });
    let mut host = ControlledHost::new_corpus_with_fonts(
        HostConfig::default(),
        SECRET.into(),
        provider.clone(),
    )
    .unwrap();
    let (controller, mut cr) = open(&mut host, 3, true, true);
    let (observer, mut vr) = open(&mut host, 4, false, false);
    assert_eq!(
        host.source_capture(&controller.client, controller.view_id)
            .unwrap()
            .resources()
            .count(),
        0
    );
    let lease = host
        .acquire_control(&controller.client, controller.view_id, [8; 16], 0)
        .unwrap();
    let raw = edit(&controller, &lease, &cr, 1, PUBLIC_TEXT_ID, Some("😀"));
    assert_eq!(
        host.submit(&controller.client, &raw, 1).unwrap().status,
        CommandStatus::Applied
    );
    assert_eq!(
        host.submit(&controller.client, &raw, 2).unwrap().status,
        CommandStatus::Applied
    );
    let controller_update = host
        .take_outbound(&controller.client, controller.view_id)
        .unwrap()
        .unwrap();
    receive(&host, &controller, &mut cr, [2; 16], &controller_update);
    let update = host
        .take_outbound(&observer.client, observer.view_id)
        .unwrap()
        .unwrap();
    assert!(!format!("{update:?}").contains(SECRET));
    receive(&host, &observer, &mut vr, [2; 16], &update);
    assert_eq!(
        host.view_description(&observer.client, observer.view_id)
            .unwrap()
            .coverage,
        "partial"
    );
    let Record::Surface(surface) = &vr.state().unwrap().records[&SURFACE_ID] else {
        panic!()
    };
    assert_eq!(
        surface.visual_coverage,
        rui_protocol_core::model::VisualCoverage::Partial
    );
    let Record::Text(text) = &vr.state().unwrap().records[&PUBLIC_TEXT_MODEL_ID] else {
        panic!()
    };
    assert_eq!(text.content.as_deref(), Some("😀"));
    assert_eq!(host.diagnostics().consumed_commands, 1);
    let unchanged_item = vr.state().unwrap().records[&101152].clone();
    let recovery = edit(&controller, &lease, &cr, 2, PUBLIC_TEXT_ID, Some("Ω"));
    host.submit(&controller.client, &recovery, 3).unwrap();
    let cb = host
        .take_outbound(&controller.client, controller.view_id)
        .unwrap()
        .unwrap();
    receive(&host, &controller, &mut cr, [3; 16], &cb);
    let ob = host
        .take_outbound(&observer.client, observer.view_id)
        .unwrap()
        .unwrap();
    receive(&host, &observer, &mut vr, [3; 16], &ob);
    assert_eq!(
        host.view_description(&observer.client, observer.view_id)
            .unwrap()
            .coverage,
        "complete"
    );
    assert_eq!(vr.state().unwrap().records[&101152], unchanged_item);
    assert_eq!(
        host.source_capture(&observer.client, observer.view_id)
            .unwrap()
            .state()
            .records,
        vr.state().unwrap().records
    );
}
#[test]
#[ignore = "native gate: requires pinned GTK/Pango/Cairo, PyGObject and bundled font"]
fn pinned_native_provider_produces_verified_outlines_and_rejects_missing_glyphs() {
    let provider = NativeSourceFontProvider::new(
        "/usr/bin/python3".into(),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/render/native_font.py"),
    );
    let shaped = provider.shape("AéΩ אב", 14.0).unwrap();
    assert!(!shaped.glyphs.is_empty());
    assert!(!shaped.resources.is_empty());
    for r in &shaped.resources {
        assert_eq!(
            r.offer().digest,
            resource_digest(
                r.offer().kind,
                r.offer().version,
                r.offer().provenance,
                r.bytes()
            )
        );
    }
    assert_eq!(provider.shape("😀", 14.0).unwrap_err(), SourceFontError);
}

struct ChangingClosure;
impl SourceFontProvider for ChangingClosure {
    fn shape(&self, text: &str, size: f64) -> Result<SourceShapedText, SourceFontError> {
        let glyph_text = match text {
            "AéΩ אב" => "A",
            "AB" => "AB",
            "C" => "C",
            _ => "",
        };
        let mut resources = Vec::new();
        let mut glyphs = Vec::new();
        for (i, c) in glyph_text.chars().enumerate() {
            let mut body = b"RVP1".to_vec();
            body.extend_from_slice(&2u32.to_be_bytes());
            body.push(1);
            body.extend_from_slice(&((c as u32) as f64 / 256.0).to_bits().to_be_bytes());
            body.extend_from_slice(&0.0f64.to_bits().to_be_bytes());
            body.push(5);
            let digest = resource_digest(
                ResourceKind::GlyphOutline,
                1,
                ResourceProvenance::FontOutline,
                &body,
            );
            resources.push(SourceOutline::new(
                ResourceOffer {
                    kind: ResourceKind::GlyphOutline,
                    version: 1,
                    provenance: ResourceProvenance::FontOutline,
                    total_bytes: body.len() as u64,
                    digest,
                },
                body,
            )?);
            glyphs.push(SourceGlyph {
                resource: digest,
                size,
                position: [i as f64, 0.0],
            });
        }
        Ok(SourceShapedText { glyphs, resources })
    }
}
#[test]
fn dropping_closure_owners_out_of_order_preserves_still_live_resource_authority() {
    let mut host = ControlledHost::new_corpus_with_fonts(
        HostConfig::default(),
        SECRET.into(),
        Arc::new(ChangingClosure),
    )
    .unwrap();
    let (controller, mut cr) = open(&mut host, 3, true, true);
    let (observer, mut vr) = open(&mut host, 4, false, false);
    let first = host
        .source_capture(&observer.client, observer.view_id)
        .unwrap();
    let a = first.resources().next().unwrap().0.digest;
    let lease = host
        .acquire_control(&controller.client, controller.view_id, [8; 16], 0)
        .unwrap();
    for (seq, text) in [(1, "AB"), (2, "C")] {
        let raw = edit(&controller, &lease, &cr, seq, PUBLIC_TEXT_ID, Some(text));
        host.submit(&controller.client, &raw, seq).unwrap();
        let cb = host
            .take_outbound(&controller.client, controller.view_id)
            .unwrap()
            .unwrap();
        receive(&host, &controller, &mut cr, [seq as u8 + 1; 16], &cb);
        let ob = host
            .take_outbound(&observer.client, observer.view_id)
            .unwrap()
            .unwrap();
        receive(&host, &observer, &mut vr, [seq as u8 + 1; 16], &ob);
    }
    // Reset history drops the newer A+B owner, while first retains older A.
    drop(host.checkpoint(&observer.client, observer.view_id).unwrap());
    assert_eq!(
        host.resource(&observer.client, observer.view_id, a)
            .unwrap()
            .offer()
            .digest,
        a
    );
    let held = host
        .retained_memory_bytes(&observer.client, observer.view_id)
        .unwrap();
    drop(first);
    assert!(host
        .resource(&observer.client, observer.view_id, a)
        .is_err());
    assert!(
        host.retained_memory_bytes(&observer.client, observer.view_id)
            .unwrap()
            < held
    );
}
#[test]
fn source_captures_and_snapshot_resources_stay_bounded_without_mutating_history_on_exhaustion() {
    let mut host = ControlledHost::new_corpus_with_fonts(
        HostConfig::default(),
        SECRET.into(),
        Arc::new(PublicProvider {
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let (s, _) = open(&mut host, 4, false, false);
    let before = host.view_status(&s.client, s.view_id).unwrap();
    let mut captures = Vec::new();
    loop {
        match host.source_capture(&s.client, s.view_id) {
            Ok(capture) => captures.push(capture),
            Err(HostError::Session(SessionError::Oversized)) => break,
            _ => panic!("unexpected bounded capture outcome"),
        }
        assert!(captures.len() < 64);
    }
    assert!(
        host.retained_memory_bytes(&s.client, s.view_id).unwrap() <= s.limits.view_memory_bytes
    );
    assert_eq!(host.view_status(&s.client, s.view_id).unwrap(), before);
    drop(captures);
    host.source_capture(&s.client, s.view_id).unwrap();
}
#[test]
#[ignore = "native gate: requires pinned GTK/Pango/Cairo, PyGObject and bundled font"]
fn native_corpus_capture_has_complete_verified_public_resource_closure() {
    let provider = NativeSourceFontProvider::new(
        "/usr/bin/python3".into(),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/render/native_font.py"),
    );
    let mut host = ControlledHost::new_corpus_with_fonts(
        HostConfig::default(),
        SECRET.into(),
        Arc::new(provider),
    )
    .unwrap();
    let (s, receiver) = open(&mut host, 4, false, false);
    assert_eq!(
        host.view_description(&s.client, s.view_id)
            .unwrap()
            .coverage,
        "complete"
    );
    let capture = host.source_capture(&s.client, s.view_id).unwrap();
    assert!(capture.resources().count() > 1);
    assert_eq!(capture.state().records, receiver.state().unwrap().records);
}
