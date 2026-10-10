//! Actual TLS/native composition probe. Presented is an explicit simulated
//! renderer callback here; the desktop GTK probe proves actual presentation.
use prost::Message;
use rui_producer_sdk::{
    gtk_host::{FixtureMutation, GtkCommandHost, GtkProducerPort},
    CaptureArtifact,
};
use rui_protocol_core::{
    commands::CommandFamily,
    model::{Record, State, TextModel},
    session::{ClientHello, Permissions, ReceiverLimits, VerifiedAuthorization},
    transcript::{RecordingPolicy, TranscriptLimits},
    wire::proto as core,
};
use rui_transport::{
    diagnostics::Diagnostics,
    pairing::{Credential, PairingAuthority},
    producer::ProducerPort,
    proto as session, ClientEvent, HostIdentity, NetworkClient, ProducerServerHandle, ServerConfig,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};
type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;
const PREEDIT: &str = "私😀e\u{301}";
const COMMIT: &str = "字😀e\u{301}";
fn hello(id: u8) -> ClientHello {
    let profiles = [
        "mirror2d.core.v1",
        "semantics.core.v1",
        "content.no-raster.v1",
        "glyph.outline-single.v1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: profiles
            .iter()
            .cloned()
            .chain(["text.clock.gtk-editable4224.v1".into()])
            .collect(),
        required_profiles: profiles,
        receiver_limits: ReceiverLimits {
            envelope_bytes: 128 << 10,
            chunk_bytes: 32 << 10,
            ..Default::default()
        },
        resume: None,
    }
}
fn approve(pairing: &mut PairingAuthority, id: u8, controller: bool) -> Result<Credential> {
    let request = pairing.request_local_pairing([id; 16])?;
    Ok(pairing.approve_local(
        request.request_id,
        VerifiedAuthorization {
            principal: format!("composition-{id}"),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: controller,
                actions: if controller {
                    ["TextComposition", "Focus", "ReplaceText", "SetSelection"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect()
                } else {
                    BTreeSet::new()
                },
            },
        },
    )?)
}
fn pinned() -> Vec<u8> {
    include_bytes!("../../../../transport/tests/data/localhost.der").to_vec()
}
async fn connect(address: SocketAddr, credential: &Credential, id: u8) -> Result<NetworkClient> {
    Ok(
        NetworkClient::connect(address, "localhost".into(), pinned(), credential, hello(id))
            .await?,
    )
}
async fn connect_after_loss(address: SocketAddr, credential: &Credential) -> Result<NetworkClient> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match connect(address, credential, 61).await {
                Ok(client) => return Ok(client),
                Err(error)
                    if matches!(
                        error.downcast_ref::<rui_transport::TransportError>(),
                        Some(rui_transport::TransportError::ProducerBusy)
                    ) =>
                {
                    // Native cleanup can finish just before the transport's
                    // old socket owner releases its single-producer slot.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await?
}
async fn installed(client: &mut NetworkClient, revision: u64) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match client.receive().await? {
                ClientEvent::Applied { revision: received } => {
                    assert_eq!(revision, received);
                    return Ok::<_, Error>(());
                }
                ClientEvent::ViewOpened(description) => {
                    assert_eq!(description.coverage, "complete");
                    if !description.granted_input_modes.is_empty() {
                        assert!(description
                            .granted_input_modes
                            .iter()
                            .any(|mode| mode == "text.client.v1"));
                    }
                }
                ClientEvent::ResourceAvailable | ClientEvent::ResourcesNeeded { .. } => {}
                other => return Err(format!("install: {other:?}").into()),
            }
        }
    })
    .await??;
    Ok(())
}
async fn result_with_revocation(
    client: &mut NetworkClient,
    revision: Option<u64>,
    expected: Option<&session::ControlGranted>,
) -> Result<session::CommandResult> {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut received = None;
        let mut installed = revision.is_none();
        loop {
            match client.receive().await? {
                ClientEvent::CommandResult(result) => received = Some(result),
                ClientEvent::ControlRevoked(revoked) => {
                    let expected = expected.ok_or("unexpected control revocation")?;
                    assert_eq!(revoked.lease_id, expected.lease_id);
                    assert_eq!(revoked.generation, expected.generation);
                    assert_eq!(revoked.reason, "NativeLifecycle");
                }
                ClientEvent::Applied { revision: actual } => {
                    assert_eq!(Some(actual), revision);
                    installed = true;
                }
                ClientEvent::ResourceAvailable | ClientEvent::ResourcesNeeded { .. } => {}
                other => return Err(format!("result: {other:?}").into()),
            }
            if installed {
                if let Some(received) = received.take() {
                    return Ok::<_, Error>(received);
                }
            }
        }
    })
    .await?
}
async fn result(
    client: &mut NetworkClient,
    revision: Option<u64>,
) -> Result<session::CommandResult> {
    result_with_revocation(client, revision, None).await
}
fn focused(state: &State) -> (&rui_protocol_core::model::SessionState, u64, &TextModel) {
    let session = state
        .records
        .values()
        .find_map(|record| match record {
            Record::Session(session) => Some(session),
            _ => None,
        })
        .unwrap();
    let focus = session.focus.unwrap().id;
    let Record::Semantic(node) = &state.records[&focus] else {
        panic!("not semantic")
    };
    let Record::Text(text) = &state.records[&node.text.unwrap().id] else {
        panic!("not text")
    };
    (session, focus, text)
}
fn composition(
    client: &NetworkClient,
    lease: &session::ControlGranted,
    sequence: u64,
    operation: core::text_composition::Operation,
) -> core::TextComposition {
    let state = client.receiver().state().unwrap();
    let (session, target, text) = focused(state);
    core::TextComposition {
        header: Some(core::CommandHeader {
            epoch: client.epoch.to_vec(),
            view_id: client.view_id.to_vec(),
            client_instance_id: [61; 16].to_vec(),
            command_seq: sequence,
            control_lease_id: lease.lease_id.clone(),
            control_generation: lease.generation,
            observed_view_revision: state.revision,
        }),
        target_id: target,
        text_session_id: [71; 16].to_vec(),
        focus_generation: session.focus_generation,
        base_text_version: text.text_version,
        operation: Some(operation),
    }
}
async fn submit(
    client: &mut NetworkClient,
    command: &core::TextComposition,
    revision: Option<u64>,
) -> Result<session::CommandResult> {
    client
        .submit_family(CommandFamily::TextComposition, &command.encode_to_vec())
        .await?;
    result(client, revision).await
}
async fn diagnostic(port: &GtkProducerPort, source: &Path) -> Result<Value> {
    port.trusted_fixture_mutation(FixtureMutation::CompositionState, 0)
        .await?;
    let bytes = fs::read(source.join("composition-state.json"))?;
    assert!(bytes.len() <= 32768);
    Ok(serde_json::from_slice(&bytes)?)
}
async fn wait_cancelled(port: &GtkProducerPort, source: &Path) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let state = diagnostic(port, source).await?;
            if state["active"] == false && state["fence_generation"] == 0 {
                return Ok::<_, Error>(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await??;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match port.flush_lifecycle(0).await {
                Ok(()) => return Ok::<_, Error>(()),
                Err(rui_transport::TransportError::ProducerBusy) => {
                    // CloseView's transport task owns the same serialized
                    // lifecycle barrier. Await its permit; commands are never
                    // retried and the native fence is already zero here.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    })
    .await??;
    Ok(())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn trace(diagnostics: &Diagnostics, destination: &Path, observer_view: [u8; 16]) -> Result<Value> {
    fs::create_dir_all(destination)?;
    let mut index = Vec::new();
    let mut total = 0usize;
    let mut observer_packets = 0usize;
    for scope in diagnostics.scopes()? {
        diagnostics.inspect(scope, |entries| -> Result<()> {
            for entry in entries {
                let bytes = entry.exact_bytes().ok_or("exact diagnostic body absent")?;
                total = total.checked_add(bytes.len()).ok_or("trace ceiling")?;
                if total > 4 << 20 { return Err("trace ceiling".into()); }
                let packet = session::Packet::decode(bytes)?;
                assert!(!matches!(session::Kind::try_from(packet.kind)?, session::Kind::ClientHello | session::Kind::ServerHello));
                let observer = scope.view_id == observer_view;
                if observer { observer_packets += 1; assert!(!bytes.windows(PREEDIT.len()).any(|window| window == PREEDIT.as_bytes())); }
                let file = format!("packet-{:04}.bin", index.len()); fs::write(destination.join(&file), bytes)?;
                index.push(json!({"file":file,"direction":format!("{:?}",entry.direction),"sequence":entry.sequence,
                    "kind":entry.kind,"wire_bytes":entry.wire_bytes,"sha256":hex(&entry.digest),"session_id":hex(&scope.scope.session_id),"epoch":hex(&scope.scope.epoch),
                    "view_id":hex(&scope.view_id),"principal_id":hex(&scope.scope.principal_id),"observer":observer}));
            } Ok(())
        })??;
    }
    fs::write(
        destination.join("index.json"),
        serde_json::to_vec_pretty(&index)?,
    )?;
    Ok(
        json!({"post_auth_packets":index.len(),"bytes":total,"observer_packets_without_preedit":observer_packets}),
    )
}
fn latest(source: &Path) -> Result<PathBuf> {
    let mut files = fs::read_dir(source)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let file = path.file_name()?.to_str()?;
            (file.starts_with("whole-") && file.ends_with(".json")).then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    files.pop().ok_or("source capture absent".into())
}
fn retain(source: &Path, destination: &Path) -> Result<PathBuf> {
    fs::create_dir_all(destination)?;
    let artifact = CaptureArtifact::read(source)?;
    let file = source.file_name().ok_or("source artifact name")?;
    let target = destination.join(file);
    fs::copy(source, &target)?;
    fs::copy(source.with_extension("bin"), target.with_extension("bin"))?;
    for offer in artifact.manifest()["resource_offers"]
        .as_array()
        .ok_or("resource descriptors")?
    {
        let file = format!("{}.rvp", offer["digest"].as_str().ok_or("resource digest")?);
        fs::copy(source.parent().unwrap().join(&file), destination.join(file))?;
    }
    Ok(target)
}
async fn renewed(
    client: &mut NetworkClient,
    lease: &session::ControlGranted,
    request: u8,
) -> Result<session::ControlGranted> {
    client
        .send(
            session::Kind::RenewControl,
            &session::RenewControl {
                request_id: [request; 16].to_vec(),
                lease_id: lease.lease_id.clone(),
                generation: lease.generation,
            },
        )
        .await?;
    match client.receive().await? {
        ClientEvent::ControlGranted(result) => Ok(result),
        other => Err(format!("renew: {other:?}").into()),
    }
}
async fn focus_entry(client: &mut NetworkClient, lease: &session::ControlGranted) -> Result<()> {
    let state = client.receiver().state()?;
    let node = state
        .records
        .values()
        .find_map(|record| match record {
            Record::Semantic(node) if node.label.as_deref() == Some("Modal entry") => Some(node),
            _ => None,
        })
        .unwrap();
    let command = core::SemanticCommand {
        epoch: client.epoch.to_vec(),
        view_id: client.view_id.to_vec(),
        client_instance_id: [61; 16].to_vec(),
        command_seq: 1,
        control_lease_id: lease.lease_id.clone(),
        control_generation: lease.generation,
        target_id: node.id,
        action_id: "Focus".into(),
        action_schema_version: 1,
        observed_view_revision: state.revision,
        preconditions: Some(core::Preconditions {
            target_version: Some(node.version),
            ..Default::default()
        }),
        arguments: Some(core::semantic_command::Arguments::NoArguments(
            core::EmptyArguments {},
        )),
    };
    client.submit(&command.encode_to_vec()).await?;
    assert_eq!(result(client, Some(2)).await?.status, "Applied");
    client.presented(2).await?;
    Ok(())
}
async fn scenario(output: &Path, name: &str) -> Result<Value> {
    fs::create_dir_all(output)?;
    let source = output.join("source");
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut native = Command::new("python3");
    native
        .arg(repository.join("fixtures/gtk/whole_native_source.py"))
        .arg("--output")
        .arg(&source)
        .arg("--fixture-hooks")
        .env("DISPLAY", std::env::var("RUI_GTK_SOURCE_DISPLAY")?)
        .env("XAUTHORITY", std::env::var("RUI_GTK_SOURCE_XAUTHORITY")?);
    let host = GtkCommandHost::spawn(native, &source)?;
    let budget = host.memory_budget();
    let working = budget.reserve(40 << 20)?;
    retain(
        host.source_artifact_for_diagnostics(),
        &output.join("source-vector"),
    )?;
    let port = Arc::new(GtkProducerPort::new(host)?);
    let mut pairing = PairingAuthority::new(4)?;
    let credential = approve(&mut pairing, 61, true)?;
    let observer_credential = approve(&mut pairing, 62, false)?;
    let pairing = Arc::new(Mutex::new(pairing));
    let diagnostics = Diagnostics::new(
        RecordingPolicy::AuthorizedDiagnosticBytes,
        TranscriptLimits {
            retained_bytes: 1 << 20,
            message_bytes: 128 << 10,
            ..Default::default()
        },
        4,
    )?;
    let identity = HostIdentity::from_der(
        pinned(),
        include_bytes!("../../../../transport/tests/data/localhost-key.der").to_vec(),
    )?;
    let server = ProducerServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity,
        port.clone(),
        pairing.clone(),
        ServerConfig {
            max_connections: 4,
            max_message_bytes: 128 << 10,
            diagnostics: Some(diagnostics.clone()),
            ..Default::default()
        },
    )
    .await?;
    let mut client = connect(server.address, &credential, 61).await?;
    assert!(client
        .profiles
        .iter()
        .any(|profile| profile == "text.clock.gtk-editable4224.v1"));
    client.open_view().await?;
    installed(&mut client, 1).await?;
    let mut observer = connect(server.address, &observer_credential, 62).await?;
    observer.open_view().await?;
    installed(&mut observer, 1).await?;
    client.presented(1).await?;
    client.acquire_control([81; 16]).await?;
    let lease = match client.receive().await? {
        ClientEvent::ControlGranted(grant) => grant,
        other => return Err(format!("grant: {other:?}").into()),
    };
    assert!(lease
        .allowed_actions
        .iter()
        .any(|action| action == "TextComposition"));
    if matches!(name, "clock-empty" | "clock-empty-noop") {
        port.trusted_fixture_mutation(FixtureMutation::CompositionEmpty, 0)
            .await?;
    }
    focus_entry(&mut client, &lease).await?;
    observer.presented(1).await?;
    installed(&mut observer, 2).await?;
    observer.presented(2).await?;
    let initial = serde_json::to_value(client.receiver().state()?)?;
    let observer_initial = serde_json::to_value(observer.receiver().state()?)?;
    let original = focused(client.receiver().state()?)
        .2
        .content
        .clone()
        .unwrap();
    let original_text_version = focused(client.receiver().state()?).2.text_version;
    let original_record_version = focused(client.receiver().state()?).2.version;
    let range = match name {
        "clock-insertion" => (1, 1),
        "clock-deletion" => (0, original.chars().count() as u64),
        "clock-prefix" => (0, 1),
        "clock-empty" | "clock-empty-noop" => (0, 0),
        _ => (1, 4),
    };
    let committed_text = match name {
        "clock-deletion" | "clock-prefix" | "clock-empty-noop" => String::new(),
        "clock-noop" => original.chars().skip(1).take(3).collect(),
        _ => COMMIT.to_owned(),
    };
    let before = retain(&latest(&source)?, &output.join("source-vector"))?;
    let before_artifact = CaptureArtifact::read(&before)?;
    before_artifact.verify_normalized_fixture_projection(client.receiver())?;
    assert!(name.starts_with("clock-empty") || original.chars().count() >= 4);
    let begin = composition(
        &client,
        &lease,
        2,
        core::text_composition::Operation::Begin(core::ScalarRange {
            start: range.0,
            end: range.1,
        }),
    );
    let begun = submit(&mut client, &begin, None).await?;
    assert_eq!(begun.status, "Dispatched");
    assert!(begun.source_commit.is_empty());
    assert!(begun.resulting_view_revision.is_none());
    assert_eq!(serde_json::to_value(client.receiver().state()?)?, initial);
    assert_eq!(
        serde_json::to_value(observer.receiver().state()?)?,
        observer_initial
    );
    let update = composition(
        &client,
        &lease,
        3,
        core::text_composition::Operation::UpdatePreedit(core::Preedit {
            text: PREEDIT.into(),
            selection: Some(core::ScalarRange { start: 1, end: 4 }),
        }),
    );
    let updated = submit(&mut client, &update, None).await?;
    assert_eq!(updated.status, "Dispatched");
    assert_eq!(serde_json::to_value(client.receiver().state()?)?, initial);
    assert_eq!(
        serde_json::to_value(observer.receiver().state()?)?,
        observer_initial
    );
    let private = diagnostic(&port, &source).await?;
    assert_eq!(private["preedit"], PREEDIT);
    assert_eq!(private["active"], true);
    let commit = composition(
        &client,
        &lease,
        4,
        core::text_composition::Operation::CommitText(core::CommitText {
            text: committed_text.clone(),
        }),
    );
    let mut evidence = json!({"scenario":name,"presentation_scope":"driver simulated Presented callback; actual TLS source/native evidence only", "transient_public_state_unchanged":true, "observer_state_unchanged":true,"private_preedit_diagnostic":private});
    assert_eq!(latest(&source)?.file_name(), before.file_name());
    before_artifact.verify_normalized_fixture_projection(observer.receiver())?;
    evidence["post_auth_before_lifecycle"] = trace(
        &diagnostics,
        &output.join("protocol-before"),
        observer.view_id,
    )?;
    evidence["source_vector_audit_before"] =
        json!("verified exact envelope/resource closure; no preedit publication or capture");
    let mut expected_dispatches = private["native_dispatches"].as_u64().unwrap();
    if name == "commit" || name == "renewal" || name.starts_with("clock-") {
        if name == "renewal" {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let first = renewed(&mut client, &lease, 82).await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            let second = renewed(&mut client, &lease, 83).await?;
            let old_retry = renewed(&mut client, &lease, 82).await?;
            assert_eq!(first, old_retry);
            assert!(second.expires_at_ms > first.expires_at_ms);
            let held = diagnostic(&port, &source).await?;
            assert_eq!(held["preedit"], PREEDIT);
            assert_eq!(held["active"], true);
            evidence["held_preedit_across_original_expiry_and_exact_old_renew"] = json!(true);
        }
        let applied = submit(&mut client, &commit, Some(3)).await?;
        assert_eq!(applied.status, "Applied");
        assert!(!applied.source_commit.is_empty());
        let after = client.receiver().state()?;
        let artifact = retain(&latest(&source)?, &output.join("source-vector"))?;
        let artifact = CaptureArtifact::read(artifact)?;
        artifact.verify_normalized_fixture_projection(client.receiver())?;
        let modal = artifact.manifest()["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|surface| surface["surface_key"] == "modal")
            .unwrap();
        assert_eq!(modal["capture_token"], applied.source_commit);
        let (session, _, text) = focused(after);
        let Record::Surface(surface) = &after.records[&session.focus_surface.unwrap().id] else {
            panic!("focus surface")
        };
        assert!(surface
            .source_capture_tokens
            .iter()
            .any(|token| token == applied.source_commit.as_bytes()));
        evidence["clock_transition"] = json!({"before_text_version":original_text_version,"after_text_version":text.text_version,
            "before_record_version":original_record_version,"after_record_version":text.version,
            "expected_caret":range.0 + committed_text.chars().count() as u64,"actual_selection":text.selection,
            "target_surface_causal_witness":true});
        let expected = original.chars().take(range.0 as usize).collect::<String>()
            + &committed_text
            + &original.chars().skip(range.1 as usize).collect::<String>();
        assert_eq!(focused(after).2.content.as_deref(), Some(expected.as_str()));
        let dispatched = diagnostic(&port, &source).await?;
        assert_eq!(dispatched["active"], false);
        let retry = submit(&mut client, &commit, None).await?;
        assert_eq!(applied, retry);
        let after_retry = diagnostic(&port, &source).await?;
        assert_eq!(
            dispatched["native_dispatches"],
            after_retry["native_dispatches"]
        );
        installed(&mut observer, 3).await?;
        assert_eq!(
            focused(observer.receiver().state()?).2.content.as_deref(),
            Some(expected.as_str())
        );
        artifact.verify_normalized_fixture_projection(observer.receiver())?;
        evidence["exact_commit_retry_equal"] = json!(true);
        evidence["native_commit_once"] = json!(true);
        evidence["causal_source_commit"] = json!(applied.source_commit);
        evidence["resulting_view_revision"] = json!(3);
        evidence["native_final"] = dispatched;
        evidence["post_auth_final"] = trace(
            &diagnostics,
            &output.join("protocol-final"),
            observer.view_id,
        )?;
        client.close().await?;
    } else {
        let mut original_client = Some(client);
        match name {
            "focus" => {
                port.trusted_fixture_mutation(FixtureMutation::CompositionFocusCycle, 0)
                    .await?;
            }
            "selection" => {
                port.trusted_fixture_mutation(FixtureMutation::SelectionCycle, 0)
                    .await?;
            }
            "removal" => {
                port.trusted_fixture_mutation(FixtureMutation::CompositionRemove, 0)
                    .await?;
            }
            "expiry" => {
                tokio::time::sleep(Duration::from_millis(5200)).await;
            }
            "cancel" => {
                let mut cancel = commit.clone();
                cancel.operation = Some(core::text_composition::Operation::Cancel(
                    core::EmptyArguments {},
                ));
                let client = original_client.as_mut().unwrap();
                let first = submit(client, &cancel, None).await?;
                assert_eq!(first.status, "Dispatched");
                assert_eq!(submit(client, &cancel, None).await?, first);
                expected_dispatches += 1;
                assert_eq!(serde_json::to_value(client.receiver().state()?)?, initial);
            }
            "revocation" => {
                pairing.lock().unwrap().revoke_local([61; 16]);
                wait_cancelled(&port, &source).await?;
            }
            "disconnect" => {
                original_client.take().unwrap().close().await?;
                wait_cancelled(&port, &source).await?;
            }
            _ => return Err("scenario unsupported".into()),
        }
        let cancelled = diagnostic(&port, &source).await?;
        assert_eq!(cancelled["active"], false);
        assert_eq!(cancelled["preedit"], Value::Null);
        if name == "disconnect" || name == "revocation" {
            let fresh_credential = if name == "revocation" {
                Some(approve(&mut pairing.lock().unwrap(), 61, true)?)
            } else {
                None
            };
            let mut fresh = connect_after_loss(
                server.address,
                fresh_credential.as_ref().unwrap_or(&credential),
            )
            .await?;
            fresh.open_view().await?;
            installed(&mut fresh, 2).await?;
            fresh.presented(2).await?;
            fresh
                .submit_family(CommandFamily::TextComposition, &commit.encode_to_vec())
                .await?;
            let denied = result(&mut fresh, None).await?;
            assert_ne!(denied.status, "Applied");
            assert_ne!(denied.status, "Dispatched");
            evidence["post_auth_final"] = trace(
                &diagnostics,
                &output.join("protocol-final"),
                observer.view_id,
            )?;
            fresh.close().await?;
            if name == "revocation" {
                let _ = original_client.take().unwrap().close().await;
            }
        } else {
            let client = original_client.as_mut().unwrap();
            let mut late = commit.clone();
            if name == "cancel" {
                late.header.as_mut().unwrap().command_seq += 1;
            }
            client
                .submit_family(CommandFamily::TextComposition, &late.encode_to_vec())
                .await?;
            let denied =
                result_with_revocation(client, None, (name == "expiry").then_some(&lease)).await?;
            assert_ne!(denied.status, "Applied");
            assert_ne!(denied.status, "Dispatched");
            assert_eq!(
                focused(client.receiver().state()?).2.content.as_deref(),
                Some(original.as_str())
            );
            evidence["post_auth_final"] = trace(
                &diagnostics,
                &output.join("protocol-final"),
                observer.view_id,
            )?;
            original_client.take().unwrap().close().await?;
        }
        let final_state = diagnostic(&port, &source).await?;
        assert_eq!(final_state["native_dispatches"], expected_dispatches);
        evidence["cancellation"] = cancelled;
        evidence["late_commit_denied_without_native_dispatch"] = json!(true);
    }
    observer.close().await?;
    server.shutdown().await;
    drop(port);
    drop(pairing);
    drop(diagnostics);
    drop(working);
    tokio::time::timeout(Duration::from_secs(3), async {
        while budget.used() != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    evidence["host_memory_after_disposal"] = json!(budget.used());
    evidence["host_reservation_peak_bytes"] = json!(budget.peak());
    Ok(evidence)
}
fn main() -> Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).ok_or("output required")?);
    let name = std::env::args().nth(2).unwrap_or_else(|| "commit".into());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let evidence = runtime.block_on(scenario(&output, &name))?;
    fs::write(
        output.join("interaction.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!("PASS: actual pinned TLS → GTK native composition; {name}; driver simulated Presented callback input only");
    Ok(())
}
