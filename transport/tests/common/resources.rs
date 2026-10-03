//! Real TLS resource lifecycle with a bounded synthetic source shaper. Actual
//! native font/library rendering is a separate explicit pinned-environment gate.
use prost::Message;
use rui_host_broker::*;
use rui_protocol_core::{
    commands::AuthenticatedClient, model::Record, resources::*, session::*, wire::proto as core,
};
use rui_transport::{
    pairing::{Credential, PairingAuthority},
    proto as p, ClientEvent, HostIdentity, NetworkClient, ServerConfig, ServerHandle,
};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};
pub const SECRET: &str = "RESOURCE-TRANSPORT-PRIVATE-SENTINEL";
struct SyntheticPublicShaper;
impl SourceFontProvider for SyntheticPublicShaper {
    fn shape(&self, text: &str, size: f64) -> Result<SourceShapedText, SourceFontError> {
        assert!(!text.contains(SECRET));
        let mut resources = BTreeMap::new();
        let mut glyphs = Vec::new();
        for (i, c) in text.chars().enumerate() {
            let mut body = b"RVP1".to_vec();
            body.extend_from_slice(&42u32.to_be_bytes());
            body.push(1);
            body.extend_from_slice(&0.0f64.to_be_bytes());
            body.extend_from_slice(&0.0f64.to_be_bytes());
            for j in 0..40 {
                body.push(2);
                body.extend_from_slice(&((c as u32) as f64 / 65536.0).to_be_bytes());
                body.extend_from_slice(&(j as f64 / 64.0).to_be_bytes());
            }
            body.push(5);
            let digest = resource_digest(
                ResourceKind::GlyphOutline,
                1,
                ResourceProvenance::FontOutline,
                &body,
            );
            resources.entry(digest).or_insert(SourceOutline::new(
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
                position: [i as f64 * 7.0, 12.0],
            });
        }
        Ok(SourceShapedText {
            glyphs,
            resources: resources.into_values().collect(),
        })
    }
}
pub fn auth(id: u8, control: bool) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: format!("resource-client-{id}"),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: control,
            actions: if control {
                ["Activate".into()].into_iter().collect()
            } else {
                Default::default()
            },
        },
    }
}
pub fn authenticated(id: u8) -> AuthenticatedClient {
    AuthenticatedClient {
        principal: format!("resource-client-{id}"),
        client_instance_id: [id; 16],
    }
}
pub fn hello(id: u8) -> ClientHello {
    let mut h = ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits {
            envelope_bytes: 1024,
            chunk_bytes: 512,
            ..Default::default()
        },
        resume: None,
    };
    h.supported_profiles
        .insert("glyph.outline-single.v1".into());
    h
}
pub async fn setup() -> (
    ServerHandle,
    Credential,
    Credential,
    Arc<Mutex<PairingAuthority>>,
) {
    let mut pairing = PairingAuthority::new(2).unwrap();
    let mut credentials = Vec::new();
    for id in [3, 4] {
        let request = pairing.request_local_pairing([id; 16]).unwrap();
        credentials.push(
            pairing
                .approve_local(request.request_id, auth(id, id == 3))
                .unwrap(),
        );
    }
    let host = ControlledHost::new_corpus_with_fonts(
        HostConfig {
            max_text_bytes: 256,
            session: SessionConfig {
                max_history_records: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        SECRET.into(),
        Arc::new(SyntheticPublicShaper),
    )
    .unwrap();
    let identity = HostIdentity::from_der(
        include_bytes!("../data/localhost.der").to_vec(),
        include_bytes!("../data/localhost-key.der").to_vec(),
    )
    .unwrap();
    let pairing = Arc::new(Mutex::new(pairing));
    let server = ServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity,
        host,
        pairing.clone(),
        ServerConfig {
            max_message_bytes: 1024,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (
        server,
        credentials.remove(0),
        credentials.remove(0),
        pairing,
    )
}
pub async fn connect(server: &ServerHandle, credential: &Credential, id: u8) -> NetworkClient {
    NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("../data/localhost.der").to_vec(),
        credential,
        hello(id),
    )
    .await
    .unwrap()
}
pub async fn applied(client: &mut NetworkClient, revision: u64) -> (usize, usize) {
    let mut need = 0;
    let mut available = 0;
    for _ in 0..1024 {
        match client.receive().await.unwrap() {
            ClientEvent::ResourcesNeeded { revision: r } => {
                assert_eq!(r, revision);
                need += 1;
            }
            ClientEvent::ResourceAvailable => available += 1,
            ClientEvent::Applied { revision: r } => {
                assert_eq!(r, revision);
                client.flush_pending().await.unwrap();
                return (need, available);
            }
            ClientEvent::CommandResult(r) => assert_eq!(r.status, "Applied"),
            ClientEvent::ViewOpened(view) => {
                assert_eq!(view.coverage, "complete");
                assert_eq!(view.canvas_width, 920.0);
            }
            other => panic!("unexpected public event {other:?}"),
        }
    }
    panic!("bounded publication did not apply")
}
pub fn activate(client: &NetworkClient, grant: &p::ControlGranted, sequence: u64) -> Vec<u8> {
    let state = client.receiver().state().unwrap();
    let Record::Semantic(node) = &state.records[&COUNTER_ID] else {
        panic!()
    };
    core::SemanticCommand {
        epoch: client.epoch.to_vec(),
        view_id: client.view_id.to_vec(),
        client_instance_id: vec![3; 16],
        command_seq: sequence,
        control_lease_id: grant.lease_id.clone(),
        control_generation: grant.generation,
        target_id: COUNTER_ID,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: state.revision,
        preconditions: Some(core::Preconditions {
            target_version: Some(node.version),
            ..Default::default()
        }),
        arguments: Some(core::semantic_command::Arguments::NoArguments(
            core::EmptyArguments {},
        )),
    }
    .encode_to_vec()
}
