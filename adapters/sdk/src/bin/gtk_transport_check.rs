//! Real pinned TLS and the single GTK owner. Presentation RPCs in this driver
//! are explicit simulated callback inputs; actual mirror probes are separate.
use prost::Message;
use rui_producer_sdk::{
    gtk_host::{FixtureMutation, GtkCommandHost, GtkProducerPort},
    CaptureArtifact,
};
use rui_protocol_core::{
    model::{Record, State},
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
const CEILING: usize = 128 << 10;
fn hello(id: u8) -> ClientHello {
    let profiles: BTreeSet<_> = [
        "mirror2d.core.v1",
        "semantics.core.v1",
        "content.no-raster.v1",
        "glyph.outline-single.v1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: profiles.clone(),
        required_profiles: profiles,
        receiver_limits: ReceiverLimits {
            envelope_bytes: CEILING,
            chunk_bytes: 32 << 10,
            ..ReceiverLimits::default()
        },
        resume: None,
    }
}
fn approve(pairing: &mut PairingAuthority, id: u8, control: bool) -> Result<Credential> {
    let request = pairing.request_local_pairing([id; 16])?;
    Ok(pairing.approve_local(
        request.request_id,
        VerifiedAuthorization {
            principal: format!("controlled-gtk-{id}"),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: control,
                actions: if control {
                    ["Activate", "Focus", "ReplaceText", "SetSelection"]
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
async fn applied(client: &mut NetworkClient, revision: u64) -> Result<()> {
    applied_after_reset(client, revision, false).await
}
async fn applied_after_reset(
    client: &mut NetworkClient,
    revision: u64,
    expected_reset: bool,
) -> Result<()> {
    let mut reset = false;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match client.receive().await? {
                ClientEvent::Applied { revision: r } => {
                    assert_eq!(r, revision);
                    assert_eq!(reset, expected_reset);
                    return Ok::<_, Error>(());
                }
                ClientEvent::ResetRequired(reason) => {
                    assert!(expected_reset && !reset);
                    assert_eq!(reason.reason, "ViewChanged");
                    reset = true;
                }
                ClientEvent::ViewOpened(view) => assert_eq!(view.coverage, "complete"),
                ClientEvent::ResourcesNeeded { .. } | ClientEvent::ResourceAvailable => {}
                other => return Err(format!("unexpected {other:?}").into()),
            }
        }
    })
    .await??;
    Ok(())
}
async fn open(client: &mut NetworkClient) -> Result<()> {
    client.open_view().await?;
    applied(client, 1).await
}
async fn grant(client: &mut NetworkClient, id: u8) -> Result<session::ControlGranted> {
    client.acquire_control([id; 16]).await?;
    match client.receive().await? {
        ClientEvent::ControlGranted(grant) => Ok(grant),
        other => Err(format!("grant: {other:?}").into()),
    }
}
fn command(client: &NetworkClient, grant: &session::ControlGranted, sequence: u64) -> Vec<u8> {
    let state = client.receiver().state().unwrap();
    let target = state
        .records
        .values()
        .find_map(|r| match r {
            Record::Semantic(node) if node.label.as_deref() == Some("Close native modal") => {
                Some(node)
            }
            _ => None,
        })
        .unwrap();
    core::SemanticCommand {
        epoch: client.epoch.to_vec(),
        view_id: client.view_id.to_vec(),
        client_instance_id: vec![41; 16],
        command_seq: sequence,
        control_lease_id: grant.lease_id.clone(),
        control_generation: grant.generation,
        target_id: target.id,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: state.revision,
        preconditions: Some(core::Preconditions {
            target_version: Some(target.version),
            ..Default::default()
        }),
        arguments: Some(core::semantic_command::Arguments::NoArguments(
            core::EmptyArguments {},
        )),
    }
    .encode_to_vec()
}
async fn result(
    client: &mut NetworkClient,
    revision: Option<u64>,
) -> Result<session::CommandResult> {
    let mut result = None;
    let mut installed = revision.is_none();
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match client.receive().await? {
                ClientEvent::CommandResult(received) => result = Some(received),
                ClientEvent::Applied { revision: r } => {
                    assert_eq!(Some(r), revision);
                    installed = true;
                }
                ClientEvent::ResourcesNeeded { .. } | ClientEvent::ResourceAvailable => {}
                other => return Err(format!("result: {other:?}").into()),
            }
            if result.is_some() && installed {
                return Ok::<_, Error>(());
            }
        }
    })
    .await??;
    Ok(result.unwrap())
}
fn modal(state: &State) -> bool {
    state
        .records
        .values()
        .any(|r| matches!(r,Record::Surface(s) if s.modal_for.is_some()&&s.mapped))
}
fn retain(source: &Path, trace: &Path) -> Result<PathBuf> {
    let artifact = CaptureArtifact::read(source)?;
    let stem = format!(
        "snapshot-{:06}",
        artifact.manifest()["revision"].as_u64().unwrap()
    );
    let target = trace.join(format!("{stem}.json"));
    fs::copy(source, &target)?;
    fs::copy(source.with_extension("bin"), target.with_extension("bin"))?;
    for offer in artifact.manifest()["resource_offers"].as_array().unwrap() {
        let name = format!("{}.rvp", offer["digest"].as_str().unwrap());
        fs::copy(source.parent().unwrap().join(&name), trace.join(name))?;
    }
    Ok(target)
}
fn latest(source: &Path) -> Result<PathBuf> {
    let mut candidates = fs::read_dir(source)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;
            if name.starts_with("whole-") && name.ends_with(".json") {
                Some(path)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop().ok_or("no current capture".into())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn trace(diagnostics: &Diagnostics, directory: &Path) -> Result<usize> {
    fs::create_dir(directory)?;
    let mut count = 0;
    let mut total = 0usize;
    let mut index = Vec::new();
    for scope in diagnostics.scopes()? {
        diagnostics.inspect(scope, |entries| -> Result<()> {
            for entry in entries {
                let bytes = entry.exact_bytes().ok_or("exact diagnostic bytes absent")?;
                total += bytes.len();
                if total > 4 << 20 {
                    return Err("diagnostic export ceiling".into());
                }
                let packet = session::Packet::decode(bytes)?;
                assert!(!matches!(
                    session::Kind::try_from(packet.kind)?,
                    session::Kind::ClientHello | session::Kind::ServerHello
                ));
                let file = format!("packet-{count:04}.bin");
                fs::write(directory.join(&file), bytes)?;
                index.push(json!({"file":file,"direction":format!("{:?}",entry.direction),"sequence":entry.sequence,"kind":entry.kind,"wire_bytes":entry.wire_bytes,"sha256":hex(&entry.digest),"session_id":hex(&scope.scope.session_id),"epoch":hex(&scope.scope.epoch),"view_id":hex(&scope.view_id),"principal_id":hex(&scope.scope.principal_id)}));
                count += 1;
            }
            Ok(())
        })??;
    }
    fs::write(
        directory.join("index.json"),
        serde_json::to_vec_pretty(&index)?,
    )?;
    Ok(count)
}
async fn receipt(path: &Path) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match fs::read(path) {
                Ok(bytes) if bytes.len() <= 1024 => match serde_json::from_slice(&bytes) {
                    Ok(value) => return Ok::<Value, Error>(value),
                    Err(error) if error.is_eof() => {}
                    Err(error) => return Err(error.into()),
                },
                Ok(_) => return Err("receipt ceiling".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await?
}
async fn scenario(root: &Path, name: &str) -> Result<Value> {
    let source = root.join("source");
    let vector = root.join("source-vector");
    let packets = root.join("protocol");
    fs::create_dir_all(&vector)?;
    fs::create_dir_all(&packets)?;
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command_source = Command::new("python3");
    command_source
        .arg(repository.join("fixtures/gtk/whole_native_source.py"))
        .arg("--output")
        .arg(&source)
        .arg("--fixture-hooks");
    let host = GtkCommandHost::spawn(command_source, &source)?;
    let budget = host.memory_budget();
    // One separately read manifest may retain16MiB, with bounded vector-body,
    // audit and exact diagnostic-export scratch. Reserve before reading any
    // additional capture beyond the source owner's charged manifest.
    let working = budget.reserve(40 << 20)?;
    let initial = retain(host.source_artifact_for_diagnostics(), &vector)?;
    let port = Arc::new(GtkProducerPort::new(host)?);
    let mut pairing = PairingAuthority::new(4)?;
    let credential = approve(&mut pairing, 41, true)?;
    let observer_credential = approve(&mut pairing, 42, false)?;
    let pairing = Arc::new(Mutex::new(pairing));
    let diagnostics = Diagnostics::new(
        RecordingPolicy::AuthorizedDiagnosticBytes,
        TranscriptLimits {
            retained_bytes: 1 << 20,
            message_bytes: CEILING,
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
            max_message_bytes: CEILING,
            diagnostics: Some(diagnostics.clone()),
            ..Default::default()
        },
    )
    .await?;
    let mut client = connect(server.address, &credential, 41)
        .await
        .map_err(|error| format!("initial connect: {error}"))?;
    open(&mut client)
        .await
        .map_err(|error| format!("initial open: {error}"))?;
    let audit = CaptureArtifact::read(&initial)?.verify_vector_audit(client.receiver())?;
    assert!(modal(client.receiver().state()?));
    // Driver-supplied simulated renderer completion, explicitly excluded from
    // the actual GUI after-paint acceptance gate.
    client.presented(1).await?;
    let lease = grant(&mut client, 51).await?;
    let old_command = command(&client, &lease, 1);
    let mut evidence = json!({"mode":name,"initial_view_revision":1,"initial_vector_audit":format!("{audit:?}"),"negotiated_limits":{"view_memory_bytes":client.limits.view_memory_bytes,"envelope_bytes":client.limits.envelope_bytes,"chunk_bytes":client.limits.chunk_bytes},"presentation_scope":"explicit driver simulated callback input; no actual GTK mirror Presented claim"});
    if name == "full-visual" {
        let mut observer = connect(server.address, &observer_credential, 42)
            .await
            .map_err(|error| format!("observer connect: {error}"))?;
        open(&mut observer)
            .await
            .map_err(|error| format!("observer open: {error}"))?;
        client.submit(&old_command).await?;
        let first = result(&mut client, Some(2)).await?;
        assert_eq!(first.status, "Applied");
        assert!(!modal(client.receiver().state()?));
        assert!(modal(observer.receiver().state()?));
        let captured = retain(&latest(&source)?, &vector)?;
        let artifact = CaptureArtifact::read(&captured)?;
        artifact.verify_vector_audit(client.receiver())?;
        assert_eq!(
            artifact.manifest()["capture_token"].as_str().unwrap(),
            first.source_commit
        );
        client.submit(&old_command).await?;
        let retry = result(&mut client, None).await?;
        assert_eq!(first, retry);
        observer.presented(1).await?;
        applied(&mut observer, 2).await?;
        assert!(!modal(observer.receiver().state()?));
        artifact.verify_vector_audit(observer.receiver())?;
        evidence["causal_source_commit"] = json!(first.source_commit);
        evidence["exact_semantic_wire_hex"] = json!(hex(&old_command));
        evidence["exact_retry_equal"] = json!(true);
        evidence["stalled_observer_then_applied_revision"] = json!(2);
        evidence["recorded_post_auth_packets"] = json!(trace(&diagnostics, &packets.join("full"))?);
        observer.close().await?;
        client.close().await?;
    } else {
        port.trusted_fixture_mutation(FixtureMutation::DelayForFence, 0)
            .await?;
        client.submit(&old_command).await?;
        let queued = receipt(&source.join("native-fence-queued.json")).await?;
        assert_eq!(queued["fence_generation"], lease.generation);
        assert_eq!(queued["native_dispatches"], 0);
        evidence["recorded_post_auth_packets_before_loss"] =
            json!(trace(&diagnostics, &packets.join("before-loss"))?);
        let old_view = client.view_id;
        let mut client = Some(client);
        let mut retained = if name == "revocation" {
            pairing.lock().unwrap().revoke_local([41; 16]);
            None
        } else {
            Some(client.take().unwrap().into_retained())
        };
        let final_guard = receipt(&source.join("native-fence-receipt.json")).await?;
        assert_eq!(
            final_guard["private_request_id"],
            queued["private_request_id"]
        );
        assert_eq!(final_guard["fence_generation"], 0);
        assert_eq!(final_guard["native_dispatches"], 0);
        assert!(
            final_guard["observed_monotonic_ns"].as_u64().unwrap()
                > queued["observed_monotonic_ns"].as_u64().unwrap()
        );
        // The final guard receipt precedes the private reply. Wait for the
        // priority disconnect to finish removing its old owner slot before
        // issuing the fresh connection; no command is redispatched here.
        port.flush_lifecycle(0).await?;
        // In the approval-revocation case the original socket stayed alive
        // through the queued→final receipt. Socket loss cannot explain that
        // guard rejection. Retain its actual local reducer only afterward.
        if let Some(client) = client.take() {
            retained = Some(client.into_retained());
        }
        let replacement = if name == "revocation" {
            Some(approve(&mut pairing.lock().unwrap(), 41, true)?)
        } else {
            None
        };
        let mut resumed = retained
            .as_mut()
            .unwrap()
            .reconnect(
                server.address,
                "localhost".into(),
                pinned(),
                replacement.as_ref().unwrap_or(&credential),
                hello(41),
            )
            .await
            .map_err(|error| format!("fresh reconnect: {error}"))?;
        assert_ne!(resumed.view_id, old_view);
        resumed.open_view().await?;
        applied_after_reset(&mut resumed, 1, true)
            .await
            .map_err(|error| format!("fresh open: {error}"))?;
        resumed.presented(1).await?;
        resumed.get_command_result(1).await?;
        let queried = result(&mut resumed, None)
            .await
            .map_err(|error| format!("fresh result query: {error}"))?;
        assert_eq!(queried.status, "Failed");
        let fresh = grant(&mut resumed, 52)
            .await
            .map_err(|error| format!("fresh grant: {error}"))?;
        assert!(fresh.generation > lease.generation);
        resumed.submit(&old_command).await?;
        let failed = result(&mut resumed, None)
            .await
            .map_err(|error| format!("old exact retry: {error}"))?;
        assert_eq!(failed.status, "Failed");
        assert_eq!(failed, queried);
        let new_command = command(&resumed, &fresh, 2);
        resumed.submit(&new_command).await?;
        let success = result(&mut resumed, Some(2)).await?;
        assert_eq!(success.status, "Applied");
        assert!(!modal(resumed.receiver().state()?));
        let captured = retain(&latest(&source)?, &vector)?;
        let artifact = CaptureArtifact::read(captured)?;
        artifact.verify_vector_audit(resumed.receiver())?;
        assert_eq!(
            artifact.manifest()["capture_token"].as_str().unwrap(),
            success.source_commit
        );
        evidence["queued_receipt"] = queued;
        evidence["final_guard_receipt"] = final_guard;
        evidence["retained_failure_exact_retry"] = json!(failed.status);
        evidence["fresh_scope_result_query_equal"] = json!(true);
        evidence["fresh_generation"] = json!(fresh.generation);
        evidence["reconnect_policy"] = json!(
            "ResetRequired(ViewChanged); new scope; actual retained client accepted fresh snapshot"
        );
        evidence["approval_revoked_while_socket_alive"] = json!(name == "revocation");
        evidence["fresh_source_commit"] = json!(success.source_commit);
        evidence["exact_old_semantic_wire_hex"] = json!(hex(&old_command));
        evidence["recorded_post_auth_packets_after_reconnect"] =
            json!(trace(&diagnostics, &packets.join("after-reconnect"))?);
        resumed.close().await?;
    }
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
    let root = PathBuf::from(std::env::args().nth(1).ok_or("output required")?);
    let name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "full-visual".into());
    if !["full-visual", "disconnect", "revocation"].contains(&name.as_str()) {
        return Err("scenario unsupported".into());
    }
    fs::create_dir_all(&root)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let report = runtime.block_on(scenario(&root, &name))?;
    fs::write(
        root.join("interaction.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("PASS: actual pinned TLS → GTK owner → native effect/lifecycle guard; {name}; simulated driver Presented input only");
    Ok(())
}
