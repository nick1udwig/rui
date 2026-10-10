//! Run only under native_check.py's isolated display/session bus.
use prost::Message;
use rui_producer_sdk::gtk_host::{FixtureMutation as Mutation, GtkCommandHost};
use rui_protocol_core::commands::{
    AuthenticatedClient, CommandFamily, CommandStatus as Status, Rejection,
};
use rui_protocol_core::model::{Record, SemanticNode, State, TextModel, TextRange};
use rui_protocol_core::session::{
    ClientHello, ControlLease, NegotiatedSession, Permissions, ReceiverLimits,
    VerifiedAuthorization,
};
use rui_protocol_core::wire::proto as p;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn semantic(state: &State, id: u64) -> &SemanticNode {
    let Record::Semantic(s) = &state.records[&id] else {
        panic!("semantic")
    };
    s
}
fn text(state: &State, id: u64) -> &TextModel {
    let Record::Text(t) = &state.records[&semantic(state, id).text.unwrap().id] else {
        panic!("text")
    };
    t
}
fn named(state: &State, label: &str) -> u64 {
    state
        .records
        .values()
        .find_map(|r| match r {
            Record::Semantic(s) if s.label.as_deref() == Some(label) => Some(s.id),
            _ => None,
        })
        .unwrap()
}
fn hello(id: u8, limits: ReceiverLimits) -> ClientHello {
    ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: [
            "mirror2d.core.v1",
            "semantics.core.v1",
            "content.no-raster.v1",
            "semantics.only.v1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        required_profiles: ["semantics.only.v1".into()].into_iter().collect(),
        receiver_limits: limits,
        resume: None,
    }
}
fn authorization(principal: &str, control: bool) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: principal.into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: control,
            actions: [
                "Activate",
                "ReplaceText",
                "SetSelection",
                "Focus",
                "ScrollTo",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        },
    }
}
fn retain_capture(host: &GtkCommandHost, directory: &Path) -> Result<()> {
    let source = host.source_artifact_for_diagnostics();
    let manifest = host.source_capture_for_diagnostics();
    let name = format!(
        "snapshot-{:02}-{:06}",
        manifest["source_generation"].as_u64().unwrap(),
        manifest["revision"].as_u64().unwrap()
    );
    let bytes = fs::read(source.with_extension("bin"))?;
    assert!(bytes.len() <= 1 << 20);
    fs::write(directory.join(format!("{name}.bin")), bytes)?;
    fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec(manifest)?,
    )?;
    for offer in manifest["resource_offers"].as_array().unwrap() {
        let resource = format!("{}.rvp", offer["digest"].as_str().unwrap());
        let destination = directory.join(&resource);
        if !destination.exists() {
            fs::copy(source.parent().unwrap().join(resource), destination)?;
        }
    }
    if manifest["local_reference"].is_object()
        && manifest["capture_boundary"]
            == "source-thread-all-registered-surfaces-fresh-snapshot-barrier"
    {
        // Diagnostic pixels have their own finite fixture quota. They never
        // enter the vector trace, protocol resources or consumer cache.
        let local = directory
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("local-reference")
            .join(directory.file_name().unwrap());
        fs::create_dir_all(&local)?;
        let revision = manifest["revision"].as_u64().unwrap();
        let label = format!("whole-{revision:04}");
        fs::copy(source, local.join(format!("{label}.json")))?;
        fs::copy(
            source.with_extension("bin"),
            local.join(format!("{label}.bin")),
        )?;
        let raster = source.with_file_name(format!("{label}.local.bgra"));
        assert!(raster.metadata()?.len() <= 4 << 20);
        fs::copy(raster, local.join(format!("{label}.local.bgra")))?;
        for offer in manifest["resource_offers"].as_array().unwrap() {
            let resource = format!("{}.rvp", offer["digest"].as_str().unwrap());
            if !local.join(&resource).exists() {
                fs::copy(
                    source.parent().unwrap().join(&resource),
                    local.join(resource),
                )?;
            }
        }
        let bytes: u64 = fs::read_dir(&local)?
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert!(bytes <= 64 << 20, "fixed local-reference quota");
    }
    // Finite diagnostic trace ownership is separate from native cache retention.
    let bytes: u64 = fs::read_dir(directory)?
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(bytes <= 64 << 20, "fixed interaction trace quota");
    Ok(())
}
fn retain_public(
    host: &GtkCommandHost,
    client: &AuthenticatedClient,
    directory: &Path,
) -> Result<()> {
    if let Some(bytes) = host.pending_publication(client)? {
        assert!(bytes.len() <= 1 << 20);
        let envelope = p::Envelope::decode(bytes)?;
        let revision = match p::MessageKind::try_from(envelope.kind)? {
            p::MessageKind::Snapshot => p::Snapshot::decode(envelope.payload.as_slice())?.revision,
            p::MessageKind::Transaction => {
                p::Transaction::decode(envelope.payload.as_slice())?.new_revision
            }
            _ => return Err("unexpected public publication".into()),
        };
        fs::write(directory.join(format!("public-{revision:06}.bin")), bytes)?;
        fs::write(
            directory.join(format!("public-{revision:06}.state.json")),
            serde_json::to_vec(host.state(client)?)?,
        )?;
    }
    Ok(())
}
struct Fixture {
    host: GtkCommandHost,
    session: NegotiatedSession,
    observers: Vec<AuthenticatedClient>,
    lease: ControlLease,
    clock: Instant,
    sequence: u64,
    trace: Vec<Value>,
    directory: PathBuf,
    trace_directory: PathBuf,
    record_public: bool,
}
impl Fixture {
    fn new(root: &Path, name: &str) -> Result<Self> {
        let directory = root.join(name);
        let trace_directory = root.join("trace").join(name);
        fs::create_dir_all(&trace_directory)?;
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut command = Command::new("python3");
        command
            .arg(repository.join(if name == "editor" {
                "demos/editor/app.py"
            } else {
                if matches!(name, "whole-native" | "native-context" | "native-fence") {
                    "fixtures/gtk/whole_native_source.py"
                } else {
                    "fixtures/gtk/native_source.py"
                }
            }))
            .arg("--output")
            .arg(&directory);
        if name != "editor" {
            command.arg("--fixture-hooks");
        }
        let clock = Instant::now();
        let mut host = GtkCommandHost::spawn(command, &directory)?;
        retain_capture(&host, &trace_directory)?;
        let session = host.negotiate(
            authorization("controlled-alice", true),
            hello(4, ReceiverLimits::default()),
            clock.elapsed().as_millis() as u64,
        )?;
        if matches!(name, "restart" | "whole-native" | "editor") {
            retain_public(&host, &session.client, &trace_directory)?;
        }
        host.consume_semantics(&session.client)?;
        let lease =
            host.acquire_control(&session.client, [8; 16], clock.elapsed().as_millis() as u64)?;
        assert!(host.presented_revision(&session.client)?.is_none());
        let state = host.state(&session.client)?;
        assert!(state.resource_refs().is_empty());
        assert!(state
            .records
            .values()
            .all(|r| !matches!(r, Record::Scene(_) | Record::Binding(_))));
        Ok(Self {
            host,
            session,
            observers: Vec::new(),
            lease,
            clock,
            sequence: 0,
            trace: Vec::new(),
            directory,
            trace_directory,
            record_public: matches!(name, "restart" | "whole-native" | "editor"),
        })
    }
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }
    fn state(&self) -> &State {
        self.host.state(&self.session.client).unwrap()
    }
    fn command(
        &mut self,
        id: u64,
        action: &str,
        args: p::semantic_command::Arguments,
    ) -> p::SemanticCommand {
        self.sequence += 1;
        let state = self.state();
        let node = semantic(state, id);
        let model = node.collection.as_ref().map(|m| m.version).or_else(|| {
            state.records.values().find_map(|r| match r {
                Record::Semantic(s) => s
                    .collection
                    .as_ref()
                    .filter(|m| m.items.iter().any(|i| i.id == id))
                    .map(|m| m.version),
                _ => None,
            })
        });
        p::SemanticCommand {
            epoch: self.session.epoch.to_vec(),
            view_id: self.session.view_id.to_vec(),
            client_instance_id: self.session.client.client_instance_id.to_vec(),
            command_seq: self.sequence,
            control_lease_id: self.lease.lease_id.to_vec(),
            control_generation: self.lease.generation,
            target_id: id,
            action_id: action.into(),
            action_schema_version: 1,
            observed_view_revision: state.revision,
            preconditions: Some(p::Preconditions {
                target_version: Some(node.version),
                text_version: if matches!(action, "ReplaceText" | "SetSelection") {
                    Some(text(state, id).text_version)
                } else {
                    None
                },
                model_version: if matches!(action, "Activate" | "ScrollTo") {
                    model
                } else {
                    None
                },
                focus_generation: None,
            }),
            arguments: Some(args),
        }
    }
    fn consume(&mut self) -> Result<()> {
        let mut clients = self.observers.clone();
        clients.push(self.session.client.clone());
        for client in clients {
            if self.host.pending_publication(&client)?.is_some() {
                if self.record_public && client == self.session.client {
                    retain_public(&self.host, &client, &self.trace_directory)?;
                }
                self.host.consume_semantics(&client)?;
            }
        }
        for observer in &self.observers {
            let state = self.host.state(observer)?;
            fn content(value: &mut serde_json::Value) {
                match value {
                    serde_json::Value::Object(fields) => {
                        for key in ["version", "text_version", "focus_generation"] {
                            fields.remove(key);
                        }
                        for field in fields.values_mut() {
                            content(field);
                        }
                    }
                    serde_json::Value::Array(fields) => {
                        for field in fields {
                            content(field);
                        }
                    }
                    _ => {}
                }
            }
            let mut observed_content = serde_json::to_value(&state.records)?;
            let mut controller = serde_json::to_value(&self.state().records)?;
            content(&mut observed_content);
            content(&mut controller);
            assert_eq!(
                observed_content, controller,
                "independent projections converge on native effects; clocks are per view"
            );
            assert!(self.host.presented_revision(observer)?.is_none());
        }
        assert!(self
            .host
            .presented_revision(&self.session.client)?
            .is_none());
        retain_capture(&self.host, &self.trace_directory)?;
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        self.host.refresh_source()?;
        self.consume()
    }
    fn submit(
        &mut self,
        label: &str,
        c: &p::SemanticCommand,
        want: Status,
    ) -> Result<rui_protocol_core::commands::CommandResult> {
        let bytes = c.encode_to_vec();
        let before = self.host.diagnostics();
        let result = self.host.submit(
            &self.session.client,
            CommandFamily::Semantic,
            &bytes,
            self.now(),
        )?;
        assert_eq!(result.status, want, "{label}: {result:?}");
        let after = self.host.diagnostics();
        let manifest = self.host.source_capture_for_diagnostics();
        let mut decoded_source = Value::Null;
        if want == Status::Applied {
            assert_eq!(
                after.native_dispatches,
                before.native_dispatches + 1,
                "{label}"
            );
            let evidence = result.evidence.as_ref().unwrap();
            assert_eq!(
                evidence.source_commit(),
                manifest["capture_token"].as_str().unwrap()
            );
            assert_eq!(
                evidence.resulting_view_revision(),
                Some(self.state().revision)
            );
            assert_eq!(
                manifest["command_cause"]["authority_gate"],
                "core-ledger-admitted"
            );
            assert!(manifest["command_cause"]["private_request_id"]
                .as_u64()
                .is_some());
            let native_bytes = fs::read(
                self.host
                    .source_artifact_for_diagnostics()
                    .with_extension("bin"),
            )?;
            assert!(native_bytes.len() <= 1 << 20);
            let envelope = p::Envelope::decode(native_bytes.as_slice())?;
            let snapshot = p::Snapshot::decode(envelope.payload.as_slice())?;
            assert_eq!(snapshot.revision, manifest["revision"].as_u64().unwrap());
            assert!(snapshot
                .records
                .iter()
                .any(|r| matches!(r.record, Some(p::record::Record::Scene(_)))));
            let mut primitives = BTreeMap::<&str, usize>::new();
            let mut surface_coverage = Vec::new();
            for record in &snapshot.records {
                match record.record.as_ref().unwrap() {
                    p::record::Record::Scene(scene) => {
                        use p::scene_node::Primitive;
                        let kind = match scene.primitive.as_ref().unwrap() {
                            Primitive::Group(_) => "Group", Primitive::SolidRect(_) => "SolidRect",
                            Primitive::FillPath(_) => "FillPath", Primitive::GlyphRun(_) => "GlyphRun",
                            Primitive::RoundedRect(_) => "RoundedRect", Primitive::Border(_) => "Border",
                            Primitive::LinearGradient(_) => "LinearGradient", Primitive::RadialGradient(_) => "RadialGradient",
                            Primitive::StrokePath(_) => "StrokePath",
                        };
                        *primitives.entry(kind).or_default() += 1;
                    }
                    p::record::Record::Surface(surface) => surface_coverage.push(json!({"id":surface.id,"mapped":surface.mapped,"visual_coverage":surface.visual_coverage,"unsupported_features":surface.unsupported_features})),
                    _ => {}
                }
            }
            decoded_source = json!({"exact_envelope_sha256":manifest["envelope_sha256"],"primitive_counts":primitives,"surfaces":surface_coverage,"resource_offers":manifest["resource_offers"],"native_capture_gaps":manifest["gaps"]});
            retain_capture(&self.host, &self.trace_directory)?;
        } else if matches!(want, Status::Rejected | Status::Failed) {
            assert_eq!(after.native_dispatches, before.native_dispatches, "{label}");
            assert!(result.evidence.is_none());
        }
        self.trace.push(json!({"case":label,"exact_semantic_wire_hex":hex(&bytes),"command_seq":c.command_seq,"target_id":c.target_id,"action":c.action_id,"observed_view_revision":c.observed_view_revision,"preconditions":{"target":c.preconditions.as_ref().unwrap().target_version,"text":c.preconditions.as_ref().unwrap().text_version,"model":c.preconditions.as_ref().unwrap().model_version},"status":format!("{:?}",result.status),"rejection":result.rejection.as_ref().map(|r|format!("{r:?}")),"source_commit":result.evidence.as_ref().map(|e|e.source_commit()),"published_revision":result.evidence.as_ref().and_then(|e|e.resulting_view_revision()),"native_dispatches":after.native_dispatches,"native_callbacks":after.native_callbacks,"exporter_counters":manifest["exporter_counters"],"decoded_full_source":decoded_source}));
        if want == Status::Applied {
            self.consume()?;
        }
        self.trace.last_mut().unwrap()["decoded_client_state"] =
            serde_json::to_value(self.state())?;
        Ok(result)
    }
    fn finish(self) -> Result<Value> {
        let budget = self.host.memory_budget();
        let retained: u64 = fs::read_dir(&self.directory)?
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert!(retained < 128 << 20);
        let captures = fs::read_dir(&self.directory)?
            .filter(|e| {
                let path = e.as_ref().unwrap().path();
                path.extension().is_some_and(|ext| ext == "json")
                    && path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("main-content-")
            })
            .count();
        if self.directory.file_name().unwrap() != "editor" {
            assert_eq!(captures, 1, "only current consumed native capture retained");
        }
        let result = json!({"commands":self.trace,"native_dispatches":self.host.diagnostics().native_dispatches,"native_callbacks":self.host.diagnostics().native_callbacks,"retained_native_artifact_bytes":retained,"host_reserved_bytes_before_drop":budget.used()});
        drop(self.host);
        assert_eq!(budget.used(), 0);
        Ok(result)
    }
    fn retry(
        &mut self,
        label: &str,
        command: &p::SemanticCommand,
        expected: &rui_protocol_core::commands::CommandResult,
    ) -> Result<()> {
        let before = self.host.diagnostics();
        let bytes = command.encode_to_vec();
        assert_eq!(
            &self.host.submit(
                &self.session.client,
                CommandFamily::Semantic,
                &bytes,
                self.now()
            )?,
            expected
        );
        assert_eq!(self.host.diagnostics(), before);
        self.trace.push(json!({"case":label,"exact_semantic_wire_hex":hex(&bytes),"command_seq":command.command_seq,"status":format!("{:?}",expected.status),"retained_result_equal":true,"received_native_counters_unchanged":true,"native_dispatches":before.native_dispatches,"native_callbacks":before.native_callbacks}));
        Ok(())
    }
}
fn none() -> p::semantic_command::Arguments {
    p::semantic_command::Arguments::NoArguments(p::EmptyArguments {})
}
fn replace(start: u64, end: u64, text: &str) -> p::semantic_command::Arguments {
    p::semantic_command::Arguments::ReplaceText(p::TextArguments {
        start,
        end,
        text: text.into(),
    })
}
fn positive_and_races(root: &Path) -> Result<Value> {
    let mut f = Fixture::new(root, "semantic")?;
    let button = named(f.state(), "Activate source action");
    let entry = named(f.state(), "Editable text");
    let beta = named(f.state(), "Beta");
    let list = named(f.state(), "Stable-ID list");
    let c = f.command(button, "Activate", none());
    let applied = f.submit("real-clicked-causal-capture", &c, Status::Applied)?;
    assert!(f.state().records.values().any(
        |r| matches!(r,Record::Semantic(s) if s.label.as_deref()==Some("Source action applied"))
    ));
    let counters = f.host.diagnostics();
    f.retry("exact-applied-retry", &c, &applied)?;
    assert_eq!(f.host.diagnostics(), counters);
    let mut changed = c.clone();
    changed.action_id = "Focus".into();
    assert!(f
        .host
        .submit(
            &f.session.client,
            CommandFamily::Semantic,
            &changed.encode_to_vec(),
            f.now()
        )
        .is_err());
    assert_eq!(f.host.diagnostics(), counters);
    let observer = f.host.negotiate(
        authorization("controlled-observer", false),
        hello(5, ReceiverLimits::default()),
        f.now(),
    )?;
    assert!(f.host.pending_publication(&f.session.client)?.is_none());
    assert_eq!(
        f.host
            .acquire_control(&f.session.client, [8; 16], f.now())?,
        f.lease
    );
    f.host.consume_semantics(&observer.client)?;
    f.observers.push(observer.client.clone());
    assert!(f
        .host
        .negotiate(
            authorization("undersized-observer", false),
            hello(
                6,
                ReceiverLimits {
                    view_memory_bytes: 256,
                    ..ReceiverLimits::default()
                }
            ),
            f.now()
        )
        .is_err());
    let mut unauthorized = c.clone();
    unauthorized.client_instance_id = observer.client.client_instance_id.to_vec();
    unauthorized.view_id = observer.view_id.to_vec();
    unauthorized.command_seq = 1;
    assert_eq!(
        f.host
            .submit(
                &observer.client,
                CommandFamily::Semantic,
                &unauthorized.encode_to_vec(),
                f.now()
            )?
            .status,
        Status::Rejected
    );
    assert_eq!(f.host.diagnostics(), counters);
    let impostor = AuthenticatedClient {
        principal: "other-principal".into(),
        client_instance_id: f.session.client.client_instance_id,
    };
    assert!(f
        .host
        .submit(
            &impostor,
            CommandFamily::Semantic,
            &c.encode_to_vec(),
            f.now()
        )
        .is_err());
    for (label, modify, want) in [
        ("wrong-view", 0, Rejection::WrongView),
        ("stale-control-generation", 1, Rejection::StaleControl),
        ("future-view", 2, Rejection::FutureRevision),
        ("stale-target", 3, Rejection::StaleTarget),
        ("stale-focus", 4, Rejection::StaleFocus),
    ] {
        let mut c = f.command(button, "Activate", none());
        match modify {
            0 => c.view_id = vec![99; 16],
            1 => c.control_generation += 1,
            2 => c.observed_view_revision += 1000,
            3 => c.preconditions.as_mut().unwrap().target_version = Some(999),
            _ => c.preconditions.as_mut().unwrap().focus_generation = Some(999),
        }
        let r = f.submit(label, &c, Status::Rejected)?;
        assert_eq!(r.rejection, Some(want));
    }
    let count = text(f.state(), entry)
        .content
        .as_ref()
        .unwrap()
        .chars()
        .count() as u64;
    let c = f.command(entry, "ReplaceText", replace(0, count, "AéΩZ"));
    f.submit("unicode-replace", &c, Status::Applied)?;
    assert_eq!(
        text(f.state(), entry).selection,
        Some(TextRange {
            anchor: 4,
            focus: 4
        })
    );
    for (insert, want, caret) in [("一", "AéΩZ一", 5), ("二", "AéΩZ一二", 6)] {
        let position = text(f.state(), entry).selection.unwrap().focus;
        let c = f.command(entry, "ReplaceText", replace(position, position, insert));
        f.submit("insert-at-returned-native-caret", &c, Status::Applied)?;
        assert_eq!(text(f.state(), entry).content.as_deref(), Some(want));
        assert_eq!(
            text(f.state(), entry).selection,
            Some(TextRange {
                anchor: caret,
                focus: caret
            })
        );
    }
    let version = text(f.state(), entry).text_version;
    let record_version = text(f.state(), entry).version;
    let c = f.command(
        entry,
        "SetSelection",
        p::semantic_command::Arguments::SetSelection(p::SelectionArguments {
            anchor: 3,
            focus: 1,
        }),
    );
    f.submit("backward-scalar-selection", &c, Status::Applied)?;
    assert_eq!(
        text(f.state(), entry).selection,
        Some(TextRange {
            anchor: 3,
            focus: 1
        })
    );
    assert_eq!(text(f.state(), entry).text_version, version);
    assert_eq!(text(f.state(), entry).version, record_version + 1);
    let c = f.command(beta, "Activate", none());
    f.submit("stable-item-activate-owner-model", &c, Status::Applied)?;
    assert_eq!(
        semantic(f.state(), list)
            .collection
            .as_ref()
            .unwrap()
            .selected_items[0]
            .id,
        beta
    );
    let old_model = semantic(f.state(), list)
        .collection
        .as_ref()
        .unwrap()
        .version;
    f.host.trusted_fixture_mutation(Mutation::RaceModel)?;
    let c = f.command(beta, "Activate", none());
    f.submit("native-materialization-before-dispatch", &c, Status::Failed)?;
    f.refresh()?;
    let mut c = f.command(beta, "Activate", none());
    c.preconditions.as_mut().unwrap().model_version = Some(old_model);
    let r = f.submit("published-stale-owner-clock", &c, Status::Rejected)?;
    assert_eq!(r.rejection, Some(Rejection::StaleModel));
    let mut c = f.command(beta, "Activate", none());
    c.preconditions.as_mut().unwrap().model_version = None;
    let r = f.submit("missing-item-owner-clock", &c, Status::Rejected)?;
    assert_eq!(r.rejection, Some(Rejection::InvalidArguments));
    let old_text = text(f.state(), entry).text_version;
    f.host.trusted_fixture_mutation(Mutation::RaceEdit)?;
    let c = f.command(entry, "ReplaceText", replace(0, 1, "x"));
    f.submit("actual-native-edit-before-snapshot", &c, Status::Failed)?;
    f.refresh()?;
    let mut c = f.command(entry, "ReplaceText", replace(0, 1, "x"));
    c.preconditions.as_mut().unwrap().text_version = Some(old_text);
    let r = f.submit("published-stale-text-clock", &c, Status::Rejected)?;
    assert_eq!(r.rejection, Some(Rejection::StaleText));
    f.host.trusted_fixture_mutation(Mutation::RaceEdit)?;
    let c = f.command(entry, "Focus", none());
    f.submit("focus-does-not-add-hidden-text-clock", &c, Status::Applied)?;
    let old_revision = f.state().revision;
    let target_version = semantic(f.state(), button).version;
    let text_before_refresh = text(f.state(), entry).clone();
    f.refresh()?;
    assert!(f.state().revision > old_revision);
    assert_eq!(semantic(f.state(), button).version, target_version);
    assert_eq!(text(f.state(), entry), &text_before_refresh);
    let mut c = f.command(button, "Activate", none());
    c.observed_view_revision = old_revision;
    f.host.trusted_fixture_mutation(Mutation::RaceVisual)?;
    f.submit(
        "old-public-revision-cannot-rebase-native-guards",
        &c,
        Status::Rejected,
    )?;
    for _ in 0..24 {
        let c = f.command(button, "Activate", none());
        f.submit("bounded-consumed-artifact-churn", &c, Status::Applied)?;
    }
    f.host.trusted_fixture_mutation(Mutation::DisableButton)?;
    f.refresh()?;
    let c = f.command(button, "Activate", none());
    let r = f.submit("disabled-native-button", &c, Status::Rejected)?;
    assert_eq!(r.rejection, Some(Rejection::DisabledTarget));
    f.host.trusted_fixture_mutation(Mutation::EnableButton)?;
    f.refresh()?;
    f.host.trusted_fixture_mutation(Mutation::RaceDestroy)?;
    let c = f.command(button, "Activate", none());
    f.submit("native-target-retired-before-dispatch", &c, Status::Failed)?;
    f.refresh()?;
    let mut c = c;
    f.sequence += 1;
    c.command_seq = f.sequence;
    let r = f.submit("retired-published-target", &c, Status::Rejected)?;
    assert_eq!(r.rejection, Some(Rejection::MissingTarget));
    let c = f.command(entry, "Focus", none());
    f.host.trusted_fixture_mutation(Mutation::DestroySurface)?;
    // Give GTK a main-loop opportunity to finish last-window shutdown. The
    // private pipe still owns the held application until lifecycle consumption
    // and late-event rejection complete; mapped windows do not own its lifetime.
    std::thread::sleep(Duration::from_millis(25));
    assert!(!f.host.poll_source(f.now())?);
    f.consume()?;
    assert!(f
        .state()
        .records
        .values()
        .filter_map(|r| match r {
            Record::Surface(s) => Some(s),
            _ => None,
        })
        .all(|s| !s.mapped));
    let r = f.submit("late-event-after-source-destroy", &c, Status::Rejected)?;
    assert_eq!(r.rejection, Some(Rejection::MissingTarget));
    f.finish()
}
fn escaped_text(root: &Path) -> Result<Value> {
    let mut f = Fixture::new(root, "escaped-text")?;
    let entry = named(f.state(), "Editable text");
    let count = text(f.state(), entry)
        .content
        .as_ref()
        .unwrap()
        .chars()
        .count() as u64;
    let c = f.command(
        entry,
        "ReplaceText",
        replace(0, count, &"\u{1}".repeat(16 << 10)),
    );
    f.submit("16k-text-96k-private-json", &c, Status::Applied)?;
    assert_eq!(
        text(f.state(), entry).content.as_ref().unwrap().len(),
        16 << 10
    );
    let c = f.command(
        entry,
        "ReplaceText",
        replace(0, 16 << 10, "restored native text"),
    );
    f.submit("restore-after-boundary-text", &c, Status::Applied)?;
    let c = f.command(entry, "ReplaceText", replace(0, 0, "\0"));
    let r = f.submit("native-nul-declined-before-dispatch", &c, Status::Rejected)?;
    assert_eq!(r.rejection, Some(Rejection::InvalidArguments));
    f.finish()
}
fn fault(root: &Path, name: &str, operation: Mutation) -> Result<Value> {
    let mut f = Fixture::new(root, name)?;
    let button = named(f.state(), "Activate source action");
    f.host.trusted_fixture_mutation(operation)?;
    let c = f.command(button, "Activate", none());
    let r = f.submit(name, &c, Status::OutcomeUnknown)?;
    assert!(r.evidence.is_none());
    let receipt: Value =
        serde_json::from_slice(&fs::read(f.directory.join("native-fault-receipt.json"))?)?;
    assert_eq!(receipt["native_dispatches"], 1);
    assert!(receipt["native_callbacks"].as_u64().unwrap() > 0);
    let count = f.host.diagnostics();
    f.retry("exact-unknown-retry", &c, &r)?;
    assert_eq!(count, f.host.diagnostics());
    assert!(f
        .host
        .acquire_control(&f.session.client, [9; 16], f.now())
        .is_err());
    // UNKNOWN makes the source unavailable and clears its records. Reuse the
    // former target guards only to prove a new sequence is denied; this is not
    // an automatic retry or a fresh editing context.
    let mut fresh = c.clone();
    f.sequence += 1;
    fresh.command_seq = f.sequence;
    let rejected = f.submit("fresh-command-after-unknown", &fresh, Status::Rejected)?;
    assert_eq!(rejected.rejection, Some(Rejection::Unauthorized));
    let budget = f.host.memory_budget();
    let trace = json!({"commands":f.trace,"local_negative_control_receipt":receipt});
    drop(f.host);
    assert_eq!(budget.used(), 0);
    Ok(trace)
}
fn expiry(root: &Path) -> Result<Value> {
    let mut f = Fixture::new(root, "delayed-expiry")?;
    let button = named(f.state(), "Activate source action");
    f.host
        .trusted_fixture_mutation(Mutation::DelayUntilExpired)?;
    std::thread::sleep(Duration::from_millis(100));
    let c = f.command(button, "Activate", none());
    let rejected = f.submit("queued-past-native-lease-expiry", &c, Status::Failed)?;
    f.retry("exact-expired-native-retry", &c, &rejected)?;
    let receipt: Value =
        serde_json::from_slice(&fs::read(f.directory.join("native-expiry-receipt.json"))?)?;
    assert_eq!(receipt["native_dispatches"], 0);
    assert_eq!(receipt["reason"], "lease-expired-at-native-dispatch");
    f.finish()
}
fn restart(root: &Path) -> Result<Value> {
    let mut f = Fixture::new(root, "restart")?;
    let host_pid = std::process::id();
    let epoch = f.host.epoch();
    let first_native_pid = f.host.native_pid();
    let first_incarnation = f.host.source_capture_for_diagnostics()["binding"]["epoch"].clone();
    let first_ids: Vec<_> = f.state().records.keys().copied().collect();
    let session_id = f
        .state()
        .records
        .values()
        .find_map(|r| {
            if let Record::Session(s) = r {
                Some(s.id)
            } else {
                None
            }
        })
        .unwrap();
    let old_button = named(f.state(), "Activate source action");
    let observer = f.host.negotiate(
        authorization("restart-observer", false),
        hello(5, ReceiverLimits::default()),
        f.now(),
    )?;
    f.host.consume_semantics(&observer.client)?;
    f.observers.push(observer.client);
    let mut old_command = f.command(old_button, "Activate", none());
    let original_command = old_command.clone();
    f.host
        .trusted_fixture_mutation(Mutation::CrashAfterDispatch)?;
    let outcome = f.submit(
        "actual-sigkill-after-native-effect",
        &old_command,
        Status::OutcomeUnknown,
    )?;
    assert_eq!(f.host.last_source_signal(), Some(libc::SIGKILL));
    assert_eq!(
        f.host.source_memory_used(),
        0,
        "dead source resources and pending capture are closed"
    );
    let receipt: Value =
        serde_json::from_slice(&fs::read(f.directory.join("native-fault-receipt.json"))?)?;
    assert_eq!(receipt["native_dispatches"], 1);
    assert!(receipt["native_callbacks"].as_u64().unwrap() > 0);
    // Exact retries remain available while the process is absent.
    f.retry(
        "same-unknown-while-source-unavailable",
        &old_command,
        &outcome,
    )?;
    let offline_observer = f.host.negotiate(
        authorization("unavailable-observer", false),
        hello(6, ReceiverLimits::default()),
        f.now(),
    )?;
    f.observers.push(offline_observer.client);
    f.consume()?;
    assert_eq!(f.state().records.len(), 1);
    let Record::Session(unavailable) = &f.state().records[&session_id] else {
        panic!("stable session")
    };
    assert!(unavailable.focus.is_none());
    let unavailable_revision = f.state().revision;
    let unavailable_state = serde_json::to_value(f.state())?;
    let replacement_directory = root.join("replacement");
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = Command::new("python3");
    command
        .arg(repository.join("fixtures/gtk/native_source.py"))
        .arg("--output")
        .arg(&replacement_directory)
        .arg("--fixture-hooks");
    f.host
        .restart_source(command, &replacement_directory, f.now())?;
    assert_eq!(
        fs::read_dir(&f.directory)?.count(),
        0,
        "old source artifact ownership disposed"
    );
    assert_eq!(std::process::id(), host_pid);
    assert_eq!(f.host.epoch(), epoch);
    assert_eq!(f.session.epoch, epoch);
    assert_ne!(f.host.native_pid(), first_native_pid);
    assert_ne!(
        f.host.source_capture_for_diagnostics()["binding"]["epoch"],
        first_incarnation
    );
    assert_eq!(f.host.source_generation(), 2);
    f.consume()?;
    let new_revision = f.state().revision;
    assert!(new_revision > unavailable_revision);
    assert!(f.state().records.contains_key(&session_id));
    for id in first_ids.iter().filter(|id| **id != session_id) {
        assert!(!f.state().records.contains_key(id));
        assert!(f.state().retired.contains(id));
    }
    let new_button = named(f.state(), "Activate source action");
    assert_ne!(new_button, old_button);
    let counts = f.host.diagnostics();
    f.retry(
        "same-precrash-unknown-after-new-source",
        &old_command,
        &outcome,
    )
    .map_err(|error| format!("restart exact retry: {error}"))?;
    assert_eq!(f.host.diagnostics(), counts);
    f.lease = f
        .host
        .acquire_control(&f.session.client, [9; 16], f.now())
        .map_err(|error| format!("restart new lease: {error}"))?;
    // A new sequence under fresh control cannot address an old app identity.
    f.sequence += 1;
    old_command.command_seq = f.sequence;
    old_command.control_lease_id = f.lease.lease_id.to_vec();
    old_command.control_generation = f.lease.generation;
    old_command.observed_view_revision = new_revision;
    let rejected = f.submit(
        "old-target-under-fresh-control",
        &old_command,
        Status::Rejected,
    )?;
    assert_eq!(rejected.rejection, Some(Rejection::MissingTarget));
    f.host
        .trusted_fixture_mutation(Mutation::RaceSourceGeneration)?;
    let stale_private = f.command(new_button, "Activate", none());
    let stale_result = f.submit(
        "retired-private-source-generation-at-native-guard",
        &stale_private,
        Status::Failed,
    )?;
    f.retry(
        "exact-failed-old-generation-retry",
        &stale_private,
        &stale_result,
    )?;
    let activate = f.command(new_button, "Activate", none());
    f.submit("new-incarnation-native-action", &activate, Status::Applied)?;
    assert!(f.state().records.values().any(
        |r| matches!(r, Record::Semantic(s) if s.label.as_deref()==Some("Source action applied"))
    ));
    let final_state = serde_json::to_value(f.state())?;
    let final_incarnation = f.host.source_capture_for_diagnostics()["binding"]["epoch"].clone();
    let final_pid = f.host.native_pid();
    let final_generation = f.host.source_generation();
    // An idle source exit is observed by the supervisor without a peer command.
    // SAFETY: this PID is the still-owned live source child of this fixture.
    assert_eq!(unsafe { libc::kill(final_pid as i32, libc::SIGKILL) }, 0);
    let exit_deadline = Instant::now() + Duration::from_millis(500);
    while !f.host.poll_source(f.now())? {
        assert!(Instant::now() < exit_deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(f.host.source_memory_used(), 0);
    f.consume()?;
    let idle_exit_state = serde_json::to_value(f.state())?;
    assert_eq!(f.state().records.len(), 1);
    assert!(f.state().revision > final_state["revision"].as_u64().unwrap());
    let before_failed_starts = f.host.publication_revision();
    assert!(f.host.refresh_source().is_err());
    assert_eq!(f.host.publication_revision(), before_failed_starts);
    assert!(f.host.pending_publication(&f.session.client)?.is_none());
    for attempt in 3..=5 {
        let bad = Command::new("/nonexistent/rui-controlled-source");
        assert!(f
            .host
            .restart_source(bad, root.join(format!("failed-start-{attempt}")), f.now())
            .is_err());
        assert_eq!(f.host.source_generation(), attempt);
        assert_eq!(f.host.epoch(), epoch);
        assert_eq!(f.host.publication_revision(), before_failed_starts);
    }
    assert!(f
        .host
        .restart_source(
            Command::new("/nonexistent/rui-controlled-source"),
            root.join("restart-limit"),
            f.now()
        )
        .is_err());
    f.retry(
        "same-unknown-after-failed-starts",
        &original_command,
        &outcome,
    )?;
    let commands = f.trace.clone();
    let budget = f.host.memory_budget();
    drop(f.host);
    assert_eq!(budget.used(), 0);
    Ok(
        json!({"broker_pid":host_pid,"broker_epoch":hex(&epoch),"first_native_pid":first_native_pid,
        "replacement_native_pid":final_pid,"first_incarnation":first_incarnation,"replacement_incarnation":final_incarnation,
        "source_generation":final_generation,"native_exit_signal":libc::SIGKILL,"old_target":old_button,"new_target":new_button,
        "old_public_ids":first_ids,"stable_session_id":session_id,"unavailable_state":unavailable_state,
        "final_state":final_state,"idle_exit_state":idle_exit_state,"failed_start_attempts":3,
        "commands":commands,"post_dispatch_receipt":receipt,"host_budget_after_drop":budget.used()}),
    )
}

fn unicode(root: &Path) -> Result<Value> {
    let mut f = Fixture::new(root, "unicode")?;
    let observer = f.host.negotiate(
        authorization("unicode-observer", false),
        hello(5, ReceiverLimits::default()),
        f.now(),
    )?;
    f.host.consume_semantics(&observer.client)?;
    f.observers.push(observer.client);
    let entry = named(f.state(), "Editable text");
    let c = f.command(entry, "Focus", none());
    f.submit("unicode-native-focus", &c, Status::Applied)?;
    let initial = text(f.state(), entry)
        .content
        .as_ref()
        .unwrap()
        .chars()
        .count() as u64;
    let mixed = "A𝄞e\u{301} אבגZ";
    assert_eq!(mixed.chars().count(), 9);
    assert!(mixed.len() > 9);
    let c = f.command(entry, "ReplaceText", replace(0, initial, mixed));
    let applied = f.submit(
        "supplementary-combining-rtl-native-replace",
        &c,
        Status::Applied,
    )?;
    assert_eq!(text(f.state(), entry).content.as_deref(), Some(mixed));
    assert_eq!(
        text(f.state(), entry).selection,
        Some(TextRange {
            anchor: 9,
            focus: 9
        })
    );
    f.retry("exact-mixed-text-retry", &c, &applied)?;
    let content_clock = text(f.state(), entry).text_version;
    for (label, anchor, focus) in [
        ("one-supplementary-scalar", 1, 2),
        ("between-base-and-combining-scalar", 2, 3),
        ("backward-rtl-scalar-selection", 8, 5),
    ] {
        let c = f.command(
            entry,
            "SetSelection",
            p::semantic_command::Arguments::SetSelection(p::SelectionArguments { anchor, focus }),
        );
        f.submit(label, &c, Status::Applied)?;
        assert_eq!(
            text(f.state(), entry).selection,
            Some(TextRange { anchor, focus })
        );
        assert_eq!(text(f.state(), entry).text_version, content_clock);
    }
    let c = f.command(entry, "ReplaceText", replace(1, 2, "𐐷"));
    f.submit("replace-one-supplementary-scalar", &c, Status::Applied)?;
    assert_eq!(
        text(f.state(), entry).content.as_deref(),
        Some("A𐐷e\u{301} אבגZ")
    );
    let mut old = f.command(entry, "ReplaceText", replace(3, 4, "\u{308}"));
    old.preconditions.as_mut().unwrap().text_version = Some(content_clock);
    let rejected = f.submit("mixed-text-stale-content-clock", &old, Status::Rejected)?;
    assert_eq!(rejected.rejection, Some(Rejection::StaleText));
    let c = f.command(entry, "ReplaceText", replace(3, 4, "\u{308}"));
    f.submit(
        "replace-combining-mark-without-byte-index",
        &c,
        Status::Applied,
    )?;
    assert_eq!(
        text(f.state(), entry).content.as_deref(),
        Some("A𐐷e\u{308} אבגZ")
    );
    assert_eq!(
        text(f.state(), entry).selection,
        Some(TextRange {
            anchor: 4,
            focus: 4
        })
    );
    let c = f.command(entry, "ReplaceText", replace(5, 8, "שלום"));
    f.submit("rtl-substring-source-logical-order", &c, Status::Applied)?;
    assert_eq!(
        text(f.state(), entry).content.as_deref(),
        Some("A𐐷e\u{308} שלוםZ")
    );
    assert_eq!(
        text(f.state(), entry).selection,
        Some(TextRange {
            anchor: 9,
            focus: 9
        })
    );
    f.finish()
}
fn whole_native(root: &Path) -> Result<Value> {
    let mut f = Fixture::new(root, "whole-native")?;
    let observer = f.host.negotiate(
        authorization("whole-observer", false),
        hello(5, ReceiverLimits::default()),
        f.now(),
    )?;
    f.host.consume_semantics(&observer.client)?;
    f.observers.push(observer.client);
    let initial = f.host.source_capture_for_diagnostics().clone();
    assert_eq!(initial["surfaces"].as_array().unwrap().len(), 4);
    assert!(initial["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["mapped"] == true));
    let modal_entry = named(f.state(), "Modal entry");
    let modal_close = named(f.state(), "Close native modal");
    let main_button = named(f.state(), "Activate source action");
    let menu = named(f.state(), "Ordinary GTK menu");
    let menu_item = named(f.state(), "Menu source action");
    let tooltip_root = named(f.state(), "Controlled tooltip root");
    let blocked = f.command(main_button, "Activate", none());
    let rejected = f.submit(
        "mapped-native-modal-blocks-main-command",
        &blocked,
        Status::Rejected,
    )?;
    assert_eq!(rejected.rejection, Some(Rejection::MissingTarget));
    let blocked_menu = f.command(menu_item, "Activate", none());
    let rejected = f.submit(
        "native-modal-blocks-main-popup-descendant",
        &blocked_menu,
        Status::Rejected,
    )?;
    assert_eq!(rejected.rejection, Some(Rejection::MissingTarget));
    let c = f.command(modal_entry, "Focus", none());
    f.submit("native-modal-focus-whole-capture", &c, Status::Applied)?;
    let count = text(f.state(), modal_entry)
        .content
        .as_ref()
        .unwrap()
        .chars()
        .count() as u64;
    let c = f.command(
        modal_entry,
        "ReplaceText",
        replace(0, count, "Modal 𝄞 é שלום"),
    );
    f.submit(
        "native-modal-unicode-edit-whole-capture",
        &c,
        Status::Applied,
    )?;
    assert_eq!(
        text(f.state(), modal_entry).content.as_deref(),
        Some("Modal 𝄞 é שלום")
    );
    assert_eq!(
        text(f.state(), modal_entry).selection,
        Some(TextRange {
            anchor: 15,
            focus: 15
        })
    );
    let c = f.command(modal_close, "Activate", none());
    let applied = f.submit(
        "native-modal-close-and-main-focus-one-capture",
        &c,
        Status::Applied,
    )?;
    f.retry("modal-close-exact-retry-no-native-call", &c, &applied)?;
    let surface = |m: &Value, key: &str| {
        m["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["surface_key"] == key)
            .unwrap()
            .clone()
    };
    let closed = f.host.source_capture_for_diagnostics();
    assert_eq!(surface(closed, "modal")["mapped"], false);
    assert!(surface(closed, "modal")["modal_for"].is_null());
    assert_eq!(surface(closed, "main-content")["focused"], true);
    assert_eq!(
        surface(&initial, "modal")["surface_id"],
        surface(closed, "modal")["surface_id"]
    );
    let c = f.command(modal_entry, "Focus", none());
    let rejected = f.submit(
        "unmapped-modal-target-rejected-before-native-call",
        &c,
        Status::Rejected,
    )?;
    assert_eq!(rejected.rejection, Some(Rejection::MissingTarget));
    // Native modality may have closed the menu. Map it through its real
    // GtkMenuButton, then close/remap it to prove stable source ownership.
    if surface(f.host.source_capture_for_diagnostics(), "menu-popup")["mapped"] != true {
        let c = f.command(menu, "Activate", none());
        f.submit("native-menu-first-map", &c, Status::Applied)?;
    }
    let mapped_menu = surface(f.host.source_capture_for_diagnostics(), "menu-popup");
    let c = f.command(menu, "Activate", none());
    f.submit("native-menu-unmap-whole-capture", &c, Status::Applied)?;
    assert_eq!(
        surface(f.host.source_capture_for_diagnostics(), "menu-popup")["mapped"],
        false
    );
    let c = f.command(menu_item, "Activate", none());
    let rejected = f.submit("unmapped-native-menu-action-rejected", &c, Status::Rejected)?;
    assert_eq!(rejected.rejection, Some(Rejection::MissingTarget));
    let c = f.command(menu, "Activate", none());
    f.submit("native-menu-remap-whole-capture", &c, Status::Applied)?;
    assert_eq!(
        mapped_menu["surface_id"],
        surface(f.host.source_capture_for_diagnostics(), "menu-popup")["surface_id"]
    );
    assert_eq!(named(f.state(), "Menu source action"), menu_item);
    let c = f.command(menu_item, "Activate", none());
    f.submit(
        "registered-native-gio-menu-action-and-close",
        &c,
        Status::Applied,
    )?;
    assert!(f.state().records.values().any(|r| matches!(r, Record::Semantic(s) if s.label.as_deref() == Some("Native menu action applied"))));
    assert_eq!(
        surface(f.host.source_capture_for_diagnostics(), "menu-popup")["mapped"],
        false
    );
    f.host
        .trusted_fixture_mutation(Mutation::InterruptWholeCapture)?;
    let c = f.command(main_button, "Activate", none());
    let applied = f.submit(
        "admitted-native-action-interrupted-barrier-restarts-without-stale-union",
        &c,
        Status::Applied,
    )?;
    f.retry(
        "interrupted-action-exact-retry-zero-new-native-calls",
        &c,
        &applied,
    )?;
    let final_capture = f.host.source_capture_for_diagnostics().clone();
    let interrupted = final_capture["interrupted_nonce"].as_u64().unwrap();
    let events = final_capture["barrier_events"].as_array().unwrap();
    assert!(events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "fresh-capture"));
    assert!(events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "abort"));
    assert!(!events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "commit"));
    assert_ne!(final_capture["capture_barrier_nonce"], interrupted);
    let stale: Vec<_> = events
        .iter()
        .filter(|e| e["nonce"] == interrupted && e["event"] == "fresh-capture")
        .map(|e| &e["token"])
        .collect();
    assert!(final_capture["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| !stale.contains(&&s["capture_token"])));
    assert!(!f.state().records.contains_key(&tooltip_root));
    assert!(final_capture["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["surface_key"] != "tooltip"));
    assert!(f.state().records.values().any(|r| matches!(r, Record::Semantic(s) if s.label.as_deref() == Some("Barrier semantic mutation retained"))));
    let mut result = f.finish()?;
    result["final_whole_capture"] = final_capture;
    result["initial_whole_capture"] = initial;
    Ok(result)
}
fn native_context(root: &Path) -> Result<Value> {
    let mut f = Fixture::new(root, "native-context")?;
    let entry = named(f.state(), "Modal entry");
    let c = f.command(entry, "Focus", none());
    f.submit("native-entry-focus-context", &c, Status::Applied)?;
    let old_focus = f.host.source_capture_for_diagnostics()["session"]["focus_generation"]
        .as_u64()
        .unwrap();
    let mut stale = f.command(entry, "Focus", none());
    stale.preconditions.as_mut().unwrap().focus_generation = Some(old_focus);
    f.host.trusted_fixture_mutation(Mutation::RaceFocusCycle)?;
    let rejected = f.submit(
        "native-away-and-back-in-one-frame-rejects-old-focus-clock",
        &stale,
        Status::Failed,
    )?;
    f.retry(
        "exact-focus-context-failure-zero-redispatch",
        &stale,
        &rejected,
    )?;
    f.refresh()?;
    let current = f.host.source_capture_for_diagnostics()["session"]["focus_generation"]
        .as_u64()
        .unwrap();
    assert!(current >= old_focus + 2);
    assert_eq!(named(f.state(), "Modal entry"), entry);
    let Record::Session(session) = f
        .state()
        .records
        .values()
        .find(|r| matches!(r, Record::Session(_)))
        .unwrap()
    else {
        panic!("session")
    };
    assert_eq!(session.focus.unwrap().id, entry);
    let c = f.command(
        entry,
        "SetSelection",
        p::semantic_command::Arguments::SetSelection(p::SelectionArguments {
            anchor: 8,
            focus: 6,
        }),
    );
    let content_version = text(f.state(), entry).text_version;
    let record_version = text(f.state(), entry).version;
    f.submit(
        "native-backward-selection-has-distinct-full-record-clock",
        &c,
        Status::Applied,
    )?;
    assert_eq!(
        text(f.state(), entry).selection,
        Some(TextRange {
            anchor: 8,
            focus: 6
        })
    );
    assert_eq!(text(f.state(), entry).text_version, content_version);
    assert!(text(f.state(), entry).version > record_version);
    let same_selection = text(f.state(), entry).selection;
    let public_before_cycle = text(f.state(), entry).version;
    let native_text_clock = |manifest: &Value| {
        manifest["semantics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == entry)
            .unwrap()["text"]["version"]
            .as_u64()
            .unwrap()
    };
    let before_cycle = native_text_clock(f.host.source_capture_for_diagnostics());
    let stale_selection = f.command(
        entry,
        "SetSelection",
        p::semantic_command::Arguments::SetSelection(p::SelectionArguments {
            anchor: 8,
            focus: 6,
        }),
    );
    f.host.trusted_fixture_mutation(Mutation::SelectionCycle)?;
    f.refresh()?;
    assert_eq!(text(f.state(), entry).selection, same_selection);
    assert_eq!(text(f.state(), entry).text_version, content_version);
    assert_eq!(text(f.state(), entry).version, public_before_cycle);
    let after_cycle = native_text_clock(f.host.source_capture_for_diagnostics());
    assert!(
        after_cycle > before_cycle,
        "native selection ABA retains its own clock"
    );
    let rejected = f.submit(
        "older-public-revision-cannot-rebase-native-selection-aba",
        &stale_selection,
        Status::Rejected,
    )?;
    assert_eq!(rejected.rejection, Some(Rejection::StaleTarget));
    let mut result = f.finish()?;
    result["text_record_before_selection_cycle"] = json!(before_cycle);
    result["text_record_after_selection_cycle"] = json!(after_cycle);
    result["content_clock_after_selection_cycle"] = json!(content_version);
    result["focus_generation_before_native_cycle"] = json!(old_focus);
    result["focus_generation_after_native_cycle"] = json!(current);
    Ok(result)
}
fn native_fence(root: &Path) -> Result<Value> {
    let mut f = Fixture::new(root, "native-fence")?;
    let close = named(f.state(), "Close native modal");
    f.host.trusted_fixture_mutation(Mutation::DelayForFence)?;
    let invalidation = f.host.invalidate_native_fence_when_queued()?;
    let delayed = f.command(close, "Activate", none());
    let failed = f.submit(
        "shared-fence-invalidated-while-native-final-guard-stalled",
        &delayed,
        Status::Failed,
    )?;
    let queued = invalidation.join().unwrap()?;
    let receipt_path = f.directory.join("native-fence-receipt.json");
    assert!(receipt_path.metadata()?.len() <= 1024);
    let receipt: Value = serde_json::from_slice(&fs::read(receipt_path)?)?;
    assert_eq!(receipt["reason"], "retired-native-lifecycle-fence");
    assert_eq!(receipt["fence_generation"], 0);
    assert_eq!(receipt["native_dispatches"], 0);
    assert_eq!(receipt["control_generation"], f.lease.generation);
    assert_eq!(queued["private_request_id"], receipt["private_request_id"]);
    assert!(
        queued["observed_monotonic_ns"].as_u64().unwrap()
            < receipt["observed_monotonic_ns"].as_u64().unwrap()
    );
    assert_eq!(f.host.diagnostics().native_dispatches, 0);
    assert!(f.host.source_capture_for_diagnostics()["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["surface_key"] == "modal" && s["mapped"] == true));
    f.retry("exact-fence-failure-zero-redispatch", &delayed, &failed)?;
    assert!(
        f.host
            .acquire_control(&f.session.client, [8; 16], f.now())
            .is_err(),
        "a retired generation cannot be revived by its exact grant retry"
    );
    f.host.release_control(
        &f.session.client,
        [9; 16],
        f.lease.lease_id,
        f.lease.generation,
        f.now(),
    )?;
    let old = f.lease.generation;
    f.lease = f
        .host
        .acquire_control(&f.session.client, [10; 16], f.now())?;
    assert!(f.lease.generation > old);
    let fresh = f.lease.generation;
    let current = f.command(close, "Activate", none());
    f.submit(
        "fresh-generation-native-action-after-fence-retirement",
        &current,
        Status::Applied,
    )?;
    assert_eq!(f.host.diagnostics().native_dispatches, 1);
    assert!(f.host.source_capture_for_diagnostics()["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["surface_key"] == "modal" && s["mapped"] == false));
    let mut report = f.finish()?;
    report["retired_generation"] = json!(old);
    report["fresh_generation"] = json!(fresh);
    report["native_fence_receipt"] = receipt;
    report["native_fence_queued"] = queued;
    report["fault_scope"] = json!(
        "trusted lifecycle-fence invalidation during a deliberately delayed admitted native command; not transport disconnect evidence"
    );
    Ok(report)
}
fn editor(root: &Path) -> Result<Value> {
    use rui_protocol_core::model::{SemanticRole, TextStatus};
    let mut f = Fixture::new(root, "editor")?;
    let find = |state: &State, role: SemanticRole, action: &str| {
        state
            .records
            .values()
            .find_map(|r| match r {
                Record::Semantic(s)
                    if s.role == role && s.actions.iter().any(|a| a.id == action) =>
                {
                    Some(s.id)
                }
                _ => None,
            })
            .unwrap()
    };
    let button = find(f.state(), SemanticRole::Button, "Activate");
    let edit = find(f.state(), SemanticRole::TextField, "ReplaceText");
    let item = f
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Semantic(s) if s.collection.is_some() => {
                s.collection.as_ref().unwrap().items.last().map(|i| i.id)
            }
            _ => None,
        })
        .unwrap();
    assert!(f.state().records.values().any(|r|matches!(r,Record::Text(t) if t.status==TextStatus::Redacted && t.content.is_none() && !t.editable)));
    assert!(!serde_json::to_string(f.state())?.contains("local private secret"));
    let c = f.command(button, "Activate", none());
    let applied = f.submit("generic-button-native-effect", &c, Status::Applied)?;
    f.retry("exact-generic-retry", &c, &applied)?;
    let focus = f.command(edit, "Focus", none());
    f.submit("generic-focus", &focus, Status::Applied)?;
    let draft = f.command(
        edit,
        "ReplaceText",
        p::semantic_command::Arguments::ReplaceText(p::TextArguments {
            start: 0,
            end: text(f.state(), edit)
                .content
                .as_ref()
                .unwrap()
                .chars()
                .count() as u64,
            text: "New document: café Ω 𝄞".into(),
        }),
    );
    f.submit("generic-unicode-edit", &draft, Status::Applied)?;
    assert_eq!(
        text(f.state(), edit).content.as_deref(),
        Some("New document: café Ω 𝄞")
    );
    let select = f.command(item, "Activate", none());
    let stale = f.command(
        edit,
        "ReplaceText",
        p::semantic_command::Arguments::ReplaceText(p::TextArguments {
            start: 0,
            end: 0,
            text: "stale".into(),
        }),
    );
    f.submit("generic-stable-item-selection", &select, Status::Applied)?;
    f.submit("draft-before-selection-rejects", &stale, Status::Rejected)?;
    assert!(f.state().records.values().any(|r|matches!(r,Record::Semantic(s) if s.collection.as_ref().is_some_and(|c|c.selected_items.iter().any(|i|i.id==item)))));
    assert!(f
        .host
        .source_capture_for_diagnostics()
        .get("local_reference")
        .is_none());
    fn descendants(state: &State, id: u64, output: &mut Vec<u64>) {
        output.push(id);
        for child in &semantic(state, id).children {
            descendants(state, child.id, output);
        }
    }
    let main_root = f
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Surface(s) if s.parent.is_none() => s.semantic_root.map(|r| r.id),
            _ => None,
        })
        .unwrap();
    let mut main_nodes = Vec::new();
    descendants(f.state(), main_root, &mut main_nodes);
    let buttons: Vec<_> = main_nodes
        .into_iter()
        .filter(|id| semantic(f.state(), *id).role == SemanticRole::Button)
        .collect();
    let open = buttons[1];
    let open_command = f.command(open, "Activate", none());
    f.submit("generic-open-dialog", &open_command, Status::Applied)?;
    let blocked = f.command(edit, "Focus", none());
    f.submit("modal-denies-main-edit", &blocked, Status::Rejected)?;
    let modal_root = f
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Surface(s) if s.mapped && s.modal_for.is_some() => {
                s.semantic_root.map(|r| r.id)
            }
            _ => None,
        })
        .unwrap();
    let mut modal_nodes = Vec::new();
    descendants(f.state(), modal_root, &mut modal_nodes);
    let close = modal_nodes
        .into_iter()
        .find(|id| semantic(f.state(), *id).role == SemanticRole::Button)
        .unwrap();
    for turn in 0..3 {
        let close_command = f.command(close, "Activate", none());
        f.submit("generic-close-dialog", &close_command, Status::Applied)?;
        if turn < 2 {
            let open_command = f.command(open, "Activate", none());
            f.submit("generic-reopen-dialog", &open_command, Status::Applied)?;
        }
    }
    let replacement = f.command(buttons[2], "Activate", none());
    let old_target = f.command(edit, "Focus", none());
    let old_ids: std::collections::BTreeSet<_> = f.state().records.keys().copied().collect();
    f.submit("generic-widget-replacement", &replacement, Status::Applied)?;
    f.submit("retired-widget-guard-denied", &old_target, Status::Rejected)?;
    let current = f
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Semantic(s)
                if s.role == SemanticRole::TextField && !old_ids.contains(&s.id) =>
            {
                Some(s.id)
            }
            _ => None,
        })
        .unwrap();
    assert_ne!(current, edit);
    f.finish()
}
fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: gtk_command_check EMPTY_ARTIFACT_DIRECTORY")?,
    );
    fs::create_dir_all(&root)?;
    let report = if std::env::args().nth(2).as_deref() == Some("editor") {
        json!({"scope":"generic registered editor controls through core ledger/native guards, coherent capture without raster diagnostics; no TLS or Presented claim", "editor":editor(&root)?})
    } else if std::env::args().nth(2).as_deref() == Some("native-fence") {
        json!({"scope":"sealed inherited atomic native lifecycle fence with actual core-admitted GTK final guard; trusted invalidation fault; no TLS, wire IME or Presented claim", "native_fence":native_fence(&root)?})
    } else if std::env::args().nth(2).as_deref() == Some("native-context") {
        json!({"scope":"actual core semantic admission and native focus/selection context guards; no TextComposition or Presented claim", "native_context":native_context(&root)?})
    } else if std::env::args().nth(2).as_deref() == Some("whole-native") {
        json!({"scope":"one core ledger to actual native GTK callbacks and coherent cooperating whole-view capture; semantic-only observers; no TLS, wire IME or Presented claim", "whole_native":whole_native(&root)?})
    } else if std::env::args().nth(2).as_deref() == Some("restart") {
        json!({"scope":"same broker/common ledger survives disposable native GTK SIGKILL/restart; semantic-only consumers, no wire IME or Presented claim","restart":restart(&root)?})
    } else if std::env::args().nth(2).as_deref() == Some("unicode") {
        json!({"scope":"actual core admission and native GTK Unicode callbacks; private isolated fixture; semantic-only observers; no wire IME or Presented claim","unicode":unicode(&root)?})
    } else {
        json!({"scope":"bounded30s private cooperating GTK fixture; one real core ledger; semantic-only clients; no TLS, raw/IME or Presented claim","semantic":positive_and_races(&root)?,"escaped_text":escaped_text(&root)?,"pipe_loss":fault(&root,"pipe-loss",Mutation::LoseAfterDispatch)?,"pipe_stall":fault(&root,"pipe-stall",Mutation::StallAfterDispatch)?,"uncorrelated_capture":fault(&root,"uncorrelated-capture",Mutation::UncorrelatedCapture)?,"delayed_expiry":expiry(&root)?})
    };
    fs::write(
        root.join("interaction.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "PASS: actual core admission → native GTK callbacks → causal capture; race/expiry/retry/unknown/retention checks; {}",
        root.join("interaction.json").display()
    );
    Ok(())
}
