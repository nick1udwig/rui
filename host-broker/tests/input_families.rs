//! Cooperating-source input tests. Presentation callbacks here are trusted test
//! callbacks, not evidence of a native GUI or operating-system input injection.
use prost::Message;
use rui_host_broker::*;
use rui_protocol_core::{
    commands::{CommandFamily, CommandResult, CommandStatus, Rejection},
    input::Event,
    model::{Primitive, Record, State, TextStatus},
    receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver},
    resources::{ResourceContext, ResourceScope},
    session::*,
    wire::proto as p,
};

const SECRET: &str = "INPUT-PRIVATE-SENTINEL";
const PREEDIT: &str = "PRIVATE-TRANSIENT-PREEDIT";
struct Peer {
    session: NegotiatedSession,
    receiver: ViewReceiver,
}
struct Fixture {
    host: ControlledHost,
    controller: Peer,
    observer: Peer,
    lease: ControlLease,
    sequence: u64,
}
fn authorization(id: u8, control: bool, read: bool) -> VerifiedAuthorization {
    let mut actions: std::collections::BTreeSet<String> = if control {
        [
            "Activate",
            "Focus",
            "ReplaceText",
            "SetSelection",
            "SetValue",
            "RawInputBatch",
            "TextComposition",
            "ResizeRequest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    } else {
        Default::default()
    };
    if read {
        actions.insert(READ_SECRET_PERMISSION.into());
    }
    VerifiedAuthorization {
        principal: format!("input-{id}"),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: control,
            actions,
        },
    }
}
fn open(host: &mut ControlledHost, id: u8, control: bool, read: bool) -> Peer {
    let session = host
        .negotiate(
            authorization(id, control, read),
            ClientHello {
                client_instance_id: [id; 16],
                supported_versions: [1].into_iter().collect::<std::collections::BTreeSet<_>>(),
                supported_profiles: SessionConfig::default().supported_profiles,
                required_profiles: SessionConfig::default().required_profiles,
                receiver_limits: ReceiverLimits::default(),
                resume: None,
            },
            0,
        )
        .unwrap();
    let mut receiver = ViewReceiver::new(
        ResourceContext {
            scope: ResourceScope {
                session_id: session.session_id,
                epoch: session.epoch,
                principal_id: session.client.client_instance_id,
            },
            view_id: session.view_id,
        },
        session.limits.clone(),
        ReceiverConfig::default(),
    )
    .unwrap();
    assert!(matches!(
        receiver
            .receive_envelope(
                [1; 16],
                &host.checkpoint(&session.client, session.view_id).unwrap()
            )
            .unwrap(),
        ReceiveOutcome::Applied { revision: 1 }
    ));
    host.acknowledge_applied(&session.client, session.view_id, 1)
        .unwrap();
    receiver.mark_presented(1).unwrap();
    host.acknowledge_presented(&session.client, session.view_id, 1)
        .unwrap();
    Peer { session, receiver }
}
impl Fixture {
    fn new(read: bool, max_text: usize) -> Self {
        let mut host = ControlledHost::new_corpus(
            HostConfig {
                max_text_bytes: max_text,
                ..Default::default()
            },
            SECRET.into(),
        )
        .unwrap();
        let controller = open(&mut host, 3, true, read);
        let observer = open(&mut host, 4, false, false);
        let lease = host
            .acquire_control(
                &controller.session.client,
                controller.session.view_id,
                [9; 16],
                0,
            )
            .unwrap();
        Self {
            host,
            controller,
            observer,
            lease,
            sequence: 0,
        }
    }
    fn state(&self) -> &State {
        self.controller.receiver.state().unwrap()
    }
    fn next(&mut self) -> p::CommandHeader {
        self.sequence += 1;
        let s = &self.controller.session;
        p::CommandHeader {
            epoch: s.epoch.to_vec(),
            view_id: s.view_id.to_vec(),
            client_instance_id: s.client.client_instance_id.to_vec(),
            command_seq: self.sequence,
            control_lease_id: self.lease.lease_id.to_vec(),
            control_generation: self.lease.generation,
            observed_view_revision: self.state().revision,
        }
    }
    fn compose(
        &mut self,
        target: u64,
        id: u8,
        operation: p::text_composition::Operation,
    ) -> p::TextComposition {
        let (focus_generation, text_version) = {
            let state = self.state();
            let focus = state
                .records
                .values()
                .find_map(|r| {
                    if let Record::Session(s) = r {
                        Some(s.focus_generation)
                    } else {
                        None
                    }
                })
                .unwrap();
            let text = match target {
                PUBLIC_TEXT_ID => PUBLIC_TEXT_MODEL_ID,
                SECRET_ID => SECRET_TEXT_ID,
                _ => panic!(),
            };
            let Record::Text(t) = &state.records[&text] else {
                panic!()
            };
            (focus, t.text_version)
        };
        p::TextComposition {
            header: Some(self.next()),
            target_id: target,
            text_session_id: vec![id; 16],
            focus_generation,
            base_text_version: text_version,
            operation: Some(operation),
        }
    }
    fn raw(&mut self, events: Vec<p::RawEvent>) -> p::RawInputBatch {
        let Record::Surface(surface) = &self.state().records[&SURFACE_ID] else {
            panic!()
        };
        let token = surface.routing_token.clone();
        p::RawInputBatch {
            header: Some(self.next()),
            surface_id: SURFACE_ID,
            routing_token: token,
            presented_revision: self.state().revision,
            seat_id: FIXTURE_SEAT.into(),
            device_id: FIXTURE_DEVICE.into(),
            key_namespace: "linux.evdev".into(),
            events,
        }
    }
    fn resize(&mut self, width: f64, height: f64) -> p::ResizeRequest {
        let Record::Surface(surface) = &self.state().records[&SURFACE_ID] else {
            panic!()
        };
        let version = surface.version;
        p::ResizeRequest {
            header: Some(self.next()),
            surface_id: SURFACE_ID,
            surface_version: version,
            width,
            height,
        }
    }
    fn semantic(&mut self, target: u64, action: &str) -> p::SemanticCommand {
        let Record::Semantic(n) = &self.state().records[&target] else {
            panic!()
        };
        let version = n.version;
        let h = self.next();
        p::SemanticCommand {
            epoch: h.epoch,
            view_id: h.view_id,
            client_instance_id: h.client_instance_id,
            command_seq: h.command_seq,
            control_lease_id: h.control_lease_id,
            control_generation: h.control_generation,
            target_id: target,
            action_id: action.into(),
            action_schema_version: 1,
            observed_view_revision: h.observed_view_revision,
            preconditions: Some(p::Preconditions {
                target_version: Some(version),
                ..Default::default()
            }),
            arguments: Some(p::semantic_command::Arguments::NoArguments(
                p::EmptyArguments {},
            )),
        }
    }
    fn submit(&mut self, family: CommandFamily, raw: &[u8]) -> CommandResult {
        let result = self
            .host
            .submit_family(&self.controller.session.client, family, raw, self.sequence)
            .unwrap();
        assert_eq!(
            self.host
                .submit_family(&self.controller.session.client, family, raw, self.sequence)
                .unwrap(),
            result
        );
        result
    }
    fn transient(&mut self, raw: &[u8]) {
        let before = self.host.diagnostics().source_commit;
        let result = self.submit(CommandFamily::TextComposition, raw);
        assert_eq!(result.status, CommandStatus::Dispatched);
        assert!(result.evidence.is_none());
        assert_eq!(self.host.diagnostics().source_commit, before);
        self.no_updates();
    }
    fn no_updates(&mut self) {
        for peer in [&self.controller, &self.observer] {
            assert!(self
                .host
                .take_outbound(&peer.session.client, peer.session.view_id)
                .unwrap()
                .is_none());
        }
    }
    fn applied(&mut self, family: CommandFamily, raw: &[u8]) {
        let before = self.host.diagnostics().source_commit;
        let result = self.submit(family, raw);
        assert_eq!(result.status, CommandStatus::Applied, "{result:?}");
        let evidence = result.evidence.unwrap();
        assert_eq!(
            evidence.source_commit(),
            format!("fixture-commit-{}", before + 1)
        );
        assert_eq!(
            evidence.resulting_view_revision(),
            Some(self.state().revision + 1)
        );
        assert_eq!(self.host.diagnostics().source_commit, before + 1);
        for peer in [&mut self.controller, &mut self.observer] {
            let revision = peer.receiver.state().unwrap().revision + 1;
            let frame = self
                .host
                .take_outbound(&peer.session.client, peer.session.view_id)
                .unwrap()
                .unwrap();
            if peer.session.client.client_instance_id == [4; 16] {
                for secret in [SECRET, PREEDIT] {
                    assert!(!frame.windows(secret.len()).any(|w| w == secret.as_bytes()));
                }
            }
            assert!(
                matches!(peer.receiver.receive_envelope((revision as u128).to_be_bytes(),&frame).unwrap(),ReceiveOutcome::Applied {revision:r} if r==revision)
            );
            self.host
                .acknowledge_applied(&peer.session.client, peer.session.view_id, revision)
                .unwrap();
            peer.receiver.mark_presented(revision).unwrap();
            self.host
                .acknowledge_presented(&peer.session.client, peer.session.view_id, revision)
                .unwrap();
            let capture = self
                .host
                .source_capture(&peer.session.client, peer.session.view_id)
                .unwrap();
            assert_eq!(
                capture.state().records,
                peer.receiver.state().unwrap().records
            );
            assert_eq!(capture.source_commit(), before + 1);
            assert!(
                self.host
                    .retained_memory_bytes(&peer.session.client, peer.session.view_id)
                    .unwrap()
                    <= peer.session.limits.view_memory_bytes
            );
        }
        self.no_updates();
    }
    fn rejected(&mut self, family: CommandFamily, raw: &[u8], why: Rejection) {
        let before = self.host.diagnostics();
        let result = self.submit(family, raw);
        assert_eq!(result.status, CommandStatus::Rejected);
        assert_eq!(result.rejection, Some(why));
        assert!(result.evidence.is_none());
        assert_eq!(self.host.diagnostics().source_commit, before.source_commit);
        self.no_updates();
    }
}
fn begin(start: u64, end: u64) -> p::text_composition::Operation {
    p::text_composition::Operation::Begin(p::ScalarRange { start, end })
}
fn commit(text: &str) -> p::text_composition::Operation {
    p::text_composition::Operation::CommitText(p::CommitText { text: text.into() })
}
fn preedit(text: &str) -> p::text_composition::Operation {
    p::text_composition::Operation::UpdatePreedit(p::Preedit {
        text: text.into(),
        selection: Some(p::ScalarRange {
            start: 0,
            end: text.chars().count() as u64,
        }),
    })
}
fn motion(timestamp: u64, x: f64, y: f64) -> p::RawEvent {
    p::RawEvent {
        timestamp_us: timestamp,
        event: Some(p::raw_event::Event::Motion(p::Point { x, y })),
    }
}
fn button(timestamp: u64, pressed: bool, x: f64, y: f64) -> p::RawEvent {
    p::RawEvent {
        timestamp_us: timestamp,
        event: Some(p::raw_event::Event::Button(p::ButtonEvent {
            code: 1,
            pressed,
            position: Some(p::Point { x, y }),
        })),
    }
}

#[test]
fn one_ledger_composes_scalar_text_moves_visible_canvas_and_resizes_real_source() {
    let mut f = Fixture::new(true, 256);
    let cmd = f.semantic(COUNTER_ID, "Activate").encode_to_vec();
    f.applied(CommandFamily::Semantic, &cmd);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, begin(1, 2)).encode_to_vec();
    f.transient(&cmd);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, preedit("😀é")).encode_to_vec();
    f.transient(&cmd);
    assert_eq!(
        f.host
            .input_preedit(&f.controller.session.client, f.controller.session.view_id)
            .unwrap(),
        Some("😀é")
    );
    let raw = f
        .raw(vec![
            button(1, true, 430.0, 200.0),
            motion(2, 480.0, 230.0),
            button(3, false, 490.0, 240.0),
        ])
        .encode_to_vec();
    f.applied(CommandFamily::RawInput, &raw);
    let Record::Scene(shape) = &f.state().records[&CANVAS_SHAPE_SCENE_ID] else {
        panic!()
    };
    let Primitive::SolidRect { rect, .. } = shape.primitive else {
        panic!()
    };
    assert_eq!((rect.x, rect.y), (490.0, 240.0));
    let trace = f
        .host
        .input_trace(&f.controller.session.client, f.controller.session.view_id)
        .unwrap();
    assert_eq!(
        trace
            .iter()
            .map(|s| (s.command_sequence, s.event_index, s.event.timestamp_us))
            .collect::<Vec<_>>(),
        vec![(4, 0, 1), (4, 1, 2), (4, 2, 3)]
    );
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, commit("😀é")).encode_to_vec();
    f.applied(CommandFamily::TextComposition, &cmd);
    let Record::Text(text) = &f.state().records[&PUBLIC_TEXT_MODEL_ID] else {
        panic!()
    };
    assert_eq!(text.content.as_deref(), Some("A😀éΩ אב"));
    assert_eq!(
        text.selection.as_ref().map(|r| (r.anchor, r.focus)),
        Some((3, 3))
    );
    let Record::Surface(surface) = &f.state().records[&SURFACE_ID] else {
        panic!()
    };
    let old_token = surface.routing_token.clone();
    let cmd = f.resize(800.0, 600.0).encode_to_vec();
    f.applied(CommandFamily::Resize, &cmd);
    let Record::Surface(surface) = &f.state().records[&SURFACE_ID] else {
        panic!()
    };
    assert_eq!(
        (surface.content_size.width, surface.content_size.height),
        (800.0, 600.0)
    );
    assert_ne!(surface.routing_token, old_token);
    let Record::Surface(second) = &f.state().records[&SECOND_SURFACE_ID] else {
        panic!()
    };
    assert!(second.desktop_rect.x >= surface.content_size.width);
    assert_eq!(f.host.diagnostics().consumed_commands, 6);
    assert_eq!(f.host.diagnostics().counter, 1);
    assert_eq!(
        f.host
            .input_preedit(&f.controller.session.client, f.controller.session.view_id)
            .unwrap(),
        None
    );
}

#[test]
fn invalid_family_preconditions_reject_before_effect_and_do_not_consume_preedit() {
    let mut f = Fixture::new(true, 32);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, begin(1, 2)).encode_to_vec();
    f.transient(&cmd);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, preedit("é")).encode_to_vec();
    f.transient(&cmd);
    let cmd = f
        .compose(PUBLIC_TEXT_ID, 7, commit(&"x".repeat(32)))
        .encode_to_vec();
    f.rejected(
        CommandFamily::TextComposition,
        &cmd,
        Rejection::InvalidArguments,
    );
    assert_eq!(
        f.host
            .input_preedit(&f.controller.session.client, f.controller.session.view_id)
            .unwrap(),
        Some("é")
    );
    let mut cmd = f.compose(PUBLIC_TEXT_ID, 7, commit("é"));
    cmd.base_text_version += 1;
    f.rejected(
        CommandFamily::TextComposition,
        &cmd.encode_to_vec(),
        Rejection::StaleText,
    );
    let mut cmd = f.raw(vec![motion(1, 430.0, 200.0)]);
    cmd.seat_id = "other-seat".into();
    f.rejected(
        CommandFamily::RawInput,
        &cmd.encode_to_vec(),
        Rejection::Unauthorized,
    );
    let mut cmd = f.raw(vec![motion(1, 430.0, 200.0)]);
    cmd.routing_token = vec![0; 16];
    f.rejected(
        CommandFamily::RawInput,
        &cmd.encode_to_vec(),
        Rejection::StaleTarget,
    );
    let cmd = f.raw(vec![motion(1, 0.0, 0.0)]).encode_to_vec();
    f.rejected(CommandFamily::RawInput, &cmd, Rejection::InvalidArguments);
    let cmd = f
        .raw(vec![p::RawEvent {
            timestamp_us: 1,
            event: Some(p::raw_event::Event::Key(p::KeyEvent {
                code: 30,
                pressed: true,
            })),
        }])
        .encode_to_vec();
    f.rejected(CommandFamily::RawInput, &cmd, Rejection::UnsupportedAction);
    let mut cmd = f.resize(800.0, 600.0);
    cmd.surface_version += 1;
    f.rejected(
        CommandFamily::Resize,
        &cmd.encode_to_vec(),
        Rejection::StaleTarget,
    );
    let cmd = f.resize(0.0, 600.0).encode_to_vec();
    f.rejected(CommandFamily::Resize, &cmd, Rejection::InvalidArguments);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, commit("é")).encode_to_vec();
    f.applied(CommandFamily::TextComposition, &cmd);
    assert!(f
        .host
        .input_trace(&f.controller.session.client, f.controller.session.view_id)
        .unwrap()
        .is_empty());
}

#[test]
fn private_preedit_is_not_projection_data_and_control_alone_cannot_edit_secret() {
    let mut f = Fixture::new(true, 256);
    let cmd = f.semantic(SECRET_ID, "Focus").encode_to_vec();
    f.applied(CommandFamily::Semantic, &cmd);
    let cmd = f.compose(SECRET_ID, 7, begin(0, 1)).encode_to_vec();
    f.transient(&cmd);
    let cmd = f.compose(SECRET_ID, 7, preedit(PREEDIT)).encode_to_vec();
    f.transient(&cmd);
    assert!(f
        .host
        .input_preedit(&f.observer.session.client, f.observer.session.view_id)
        .is_err());
    let snap = f
        .host
        .snapshot(&f.observer.session.client, f.observer.session.view_id)
        .unwrap();
    for secret in [SECRET, PREEDIT] {
        assert!(!snap.windows(secret.len()).any(|w| w == secret.as_bytes()));
    }
    assert!(!format!("{:?}", f.host.diagnostics()).contains(PREEDIT));
    let cmd = f.compose(SECRET_ID, 7, commit("é")).encode_to_vec();
    f.applied(CommandFamily::TextComposition, &cmd);
    let Record::Text(private) = &f.observer.receiver.state().unwrap().records[&SECRET_TEXT_ID]
    else {
        panic!()
    };
    assert_eq!(private.status, TextStatus::Redacted);
    assert!(private.content.is_none());
    let mut unread = Fixture::new(false, 256);
    let cmd = unread.semantic(SECRET_ID, "Focus").encode_to_vec();
    unread.applied(CommandFamily::Semantic, &cmd);
    let cmd = unread.compose(SECRET_ID, 7, begin(0, 1)).encode_to_vec();
    unread.rejected(
        CommandFamily::TextComposition,
        &cmd,
        Rejection::Unauthorized,
    );
    let Record::Text(private) = &unread.state().records[&SECRET_TEXT_ID] else {
        panic!()
    };
    assert!(!private.editable);
    assert!(private.content.is_none());
}

#[test]
fn focus_local_takeover_and_disconnect_unwind_transient_and_final_held_union() {
    let mut f = Fixture::new(true, 256);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, begin(0, 0)).encode_to_vec();
    f.transient(&cmd);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, preedit("é")).encode_to_vec();
    f.transient(&cmd);
    let cmd = f.semantic(SECRET_ID, "Focus").encode_to_vec();
    f.applied(CommandFamily::Semantic, &cmd);
    assert_eq!(
        f.host
            .input_preedit(&f.controller.session.client, f.controller.session.view_id)
            .unwrap(),
        None
    );
    let mut stale = f.compose(PUBLIC_TEXT_ID, 7, commit("é"));
    stale.focus_generation -= 1;
    f.rejected(
        CommandFamily::TextComposition,
        &stale.encode_to_vec(),
        Rejection::StaleFocus,
    );
    let cmd = f
        .raw(vec![
            button(1, true, 430.0, 200.0),
            button(2, false, 440.0, 210.0),
            button(3, true, 450.0, 220.0),
        ])
        .encode_to_vec();
    f.applied(CommandFamily::RawInput, &cmd);
    f.host.local_activity(100);
    let cleanup = f.host.take_cleanup().unwrap();
    assert_eq!(cleanup.buttons, vec![1]);
    assert!(cleanup.keys.is_empty());
    assert!(f
        .host
        .input_trace(&f.controller.session.client, f.controller.session.view_id)
        .unwrap()
        .is_empty());
    // Retry returns the original source evidence after lease loss, without a
    // new event or source capture; a fresh sequence with the old lease rejects.
    assert_eq!(
        f.host
            .submit_family(
                &f.controller.session.client,
                CommandFamily::RawInput,
                &cmd,
                101
            )
            .unwrap()
            .status,
        CommandStatus::Applied
    );
    let cmd = f.raw(vec![motion(4, 460.0, 230.0)]).encode_to_vec();
    f.rejected(CommandFamily::RawInput, &cmd, Rejection::StaleControl);
    f.lease = f
        .host
        .acquire_control(
            &f.controller.session.client,
            f.controller.session.view_id,
            [10; 16],
            102,
        )
        .unwrap();
    let cmd = f.raw(vec![button(5, true, 430.0, 200.0)]).encode_to_vec();
    f.applied(CommandFamily::RawInput, &cmd);
    f.host
        .disconnect(&f.controller.session.client, 103)
        .unwrap();
    let cleanup = f.host.take_cleanup().unwrap();
    assert_eq!(cleanup.buttons, vec![1]);
}

#[test]
fn bounded_trace_preserves_every_ordered_sample_and_rejects_overflow_before_dispatch() {
    let mut f = Fixture::new(false, 256);
    for batch in 0..4u64 {
        let cmd = f
            .raw(
                (0..256)
                    .map(|i| motion(batch * 256 + i + 1, 430.0 + (i % 100) as f64, 200.0))
                    .collect(),
            )
            .encode_to_vec();
        f.applied(CommandFamily::RawInput, &cmd);
    }
    let trace = f
        .host
        .input_trace(&f.controller.session.client, f.controller.session.view_id)
        .unwrap();
    assert_eq!(trace.len(), MAX_RAW_SAMPLES);
    for (i, sample) in trace.iter().enumerate() {
        assert_eq!(sample.command_sequence, (i / 256 + 1) as u64);
        assert_eq!(sample.event_index, (i % 256) as u16);
        assert_eq!(sample.event.timestamp_us, (i + 1) as u64);
        assert!(matches!(sample.event.event, Event::Motion(_)));
    }
    let cmd = f.raw(vec![motion(1025, 450.0, 200.0)]).encode_to_vec();
    f.rejected(CommandFamily::RawInput, &cmd, Rejection::InvalidArguments);
    assert_eq!(
        f.host
            .input_trace(&f.controller.session.client, f.controller.session.view_id)
            .unwrap()
            .len(),
        MAX_RAW_SAMPLES
    );
}

#[test]
fn controller_cache_loss_neutralizes_input_without_affecting_other_views() {
    let mut f = Fixture::new(true, 256);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, begin(0, 0)).encode_to_vec();
    f.transient(&cmd);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, preedit("é")).encode_to_vec();
    f.transient(&cmd);
    let down = p::RawEvent {
        timestamp_us: 1,
        event: Some(p::raw_event::Event::TouchDown(p::TouchEvent {
            id: 1,
            position: Some(p::Point { x: 430.0, y: 200.0 }),
        })),
    };
    let cmd = f.raw(vec![down]).encode_to_vec();
    f.applied(CommandFamily::RawInput, &cmd);
    f.host
        .mark_resources_missing(&f.observer.session.client, f.observer.session.view_id)
        .unwrap();
    assert_eq!(
        f.host
            .input_preedit(&f.controller.session.client, f.controller.session.view_id)
            .unwrap(),
        Some("é")
    );
    f.host
        .mark_resources_missing(&f.controller.session.client, f.controller.session.view_id)
        .unwrap();
    assert_eq!(
        f.host
            .input_preedit(&f.controller.session.client, f.controller.session.view_id)
            .unwrap(),
        None
    );
    let revision = f.state().revision;
    for peer in [&f.controller, &f.observer] {
        f.host
            .acknowledge_applied(&peer.session.client, peer.session.view_id, revision)
            .unwrap();
        f.host
            .acknowledge_presented(&peer.session.client, peer.session.view_id, revision)
            .unwrap();
    }
    let cmd = f
        .compose(PUBLIC_TEXT_ID, 7, preedit("stale"))
        .encode_to_vec();
    f.rejected(CommandFamily::TextComposition, &cmd, Rejection::StaleText);
    let move_event = p::RawEvent {
        timestamp_us: 2,
        event: Some(p::raw_event::Event::TouchMove(p::TouchEvent {
            id: 1,
            position: Some(p::Point { x: 480.0, y: 220.0 }),
        })),
    };
    let cmd = f.raw(vec![move_event]).encode_to_vec();
    f.rejected(CommandFamily::RawInput, &cmd, Rejection::InvalidArguments);
    // With drag neutralized, a new motion does not move the retained source
    // shape. A fresh composition identity works without restoring the old one.
    let before = f.state().records[&CANVAS_SHAPE_SCENE_ID].clone();
    let cmd = f.raw(vec![motion(3, 490.0, 230.0)]).encode_to_vec();
    f.applied(CommandFamily::RawInput, &cmd);
    assert_eq!(f.state().records[&CANVAS_SHAPE_SCENE_ID], before);
    let cmd = f.compose(PUBLIC_TEXT_ID, 8, begin(0, 0)).encode_to_vec();
    f.transient(&cmd);
    f.host.local_activity(100);
    let cleanup = f.host.take_cleanup().unwrap();
    assert!(cleanup.touches.is_empty());
    assert_eq!(cleanup.composition, Some([8; 16]));
}

#[test]
fn input_owner_is_reserved_before_control_and_failed_admission_does_not_mint_lease() {
    let mut host = ControlledHost::new_corpus(
        HostConfig {
            max_text_bytes: 256,
            ..Default::default()
        },
        SECRET.into(),
    )
    .unwrap();
    let peer = open(&mut host, 3, true, false);
    let before = host
        .retained_memory_bytes(&peer.session.client, peer.session.view_id)
        .unwrap();
    let held = host
        .reserve_view_memory(
            &peer.session.client,
            peer.session.view_id,
            peer.session.limits.view_memory_bytes - before - 1,
        )
        .unwrap();
    assert_eq!(
        host.acquire_control(&peer.session.client, peer.session.view_id, [9; 16], 0),
        Err(HostError::Session(SessionError::Oversized))
    );
    assert_eq!(
        host.retained_memory_bytes(&peer.session.client, peer.session.view_id)
            .unwrap(),
        peer.session.limits.view_memory_bytes - 1
    );
    drop(held);
    let lease = host
        .acquire_control(&peer.session.client, peer.session.view_id, [9; 16], 0)
        .unwrap();
    assert_eq!(lease.generation, 1);
    assert_eq!(
        host.acquire_control(&peer.session.client, peer.session.view_id, [9; 16], 0)
            .unwrap(),
        lease
    );
}

#[test]
fn late_invalid_event_modal_block_and_expiry_cannot_leave_partial_source_effects() {
    let mut f = Fixture::new(true, 256);
    let before = f.state().records[&CANVAS_SHAPE_SCENE_ID].clone();
    let cmd = f
        .raw(vec![button(1, true, 430.0, 200.0), motion(0, 450.0, 220.0)])
        .encode_to_vec();
    f.rejected(CommandFamily::RawInput, &cmd, Rejection::InvalidArguments);
    assert_eq!(f.state().records[&CANVAS_SHAPE_SCENE_ID], before);
    assert!(f
        .host
        .input_trace(&f.controller.session.client, f.controller.session.view_id)
        .unwrap()
        .is_empty());
    // The rejected candidate did not consume a timestamp or held transition.
    let cmd = f
        .raw(vec![
            button(1, true, 430.0, 200.0),
            button(2, false, 440.0, 210.0),
        ])
        .encode_to_vec();
    f.applied(CommandFamily::RawInput, &cmd);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, begin(0, 0)).encode_to_vec();
    f.transient(&cmd);
    let cmd = f
        .compose(
            PUBLIC_TEXT_ID,
            7,
            p::text_composition::Operation::Cancel(p::EmptyArguments {}),
        )
        .encode_to_vec();
    f.transient(&cmd);
    let cmd = f.compose(PUBLIC_TEXT_ID, 7, begin(0, 0)).encode_to_vec();
    f.rejected(CommandFamily::TextComposition, &cmd, Rejection::StaleText);
    let cmd = f.compose(PUBLIC_TEXT_ID, 8, begin(0, 0)).encode_to_vec();
    f.transient(&cmd);
    let cmd = f.raw(vec![button(3, true, 450.0, 220.0)]).encode_to_vec();
    f.applied(CommandFamily::RawInput, &cmd);
    f.host.tick(5000);
    let cleanup = f.host.take_cleanup().unwrap();
    assert_eq!(cleanup.buttons, vec![1]);
    assert_eq!(cleanup.composition, Some([8; 16]));
    assert_eq!(
        f.host
            .input_preedit(&f.controller.session.client, f.controller.session.view_id)
            .unwrap(),
        None
    );
    f.lease = f
        .host
        .acquire_control(
            &f.controller.session.client,
            f.controller.session.view_id,
            [10; 16],
            5001,
        )
        .unwrap();
    let cmd = f.semantic(MODAL_TOGGLE_ID, "Activate").encode_to_vec();
    f.applied(CommandFamily::Semantic, &cmd);
    let cmd = f.raw(vec![motion(4, 460.0, 230.0)]).encode_to_vec();
    f.rejected(CommandFamily::RawInput, &cmd, Rejection::Unauthorized);
    let cmd = f.resize(800.0, 600.0).encode_to_vec();
    f.rejected(CommandFamily::Resize, &cmd, Rejection::DisabledTarget);
}
