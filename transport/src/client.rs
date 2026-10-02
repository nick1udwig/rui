use crate::{codec, pairing::Credential, proto as p, tls, websocket, TransportError};
use prost::Message;
use rui_protocol_core::commands::Id;
use rui_protocol_core::receiver::{NeededResources, ReceiveOutcome, ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{ResourceContext, ResourceScope};
use rui_protocol_core::session::{ClientHello, ReceiverLimits};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::WebSocketStream;
use zeroize::Zeroize;

#[derive(Debug)]
pub enum ClientEvent {
    ViewOpened(p::ViewOpened),
    Applied { revision: u64 },
    ResourcesNeeded { revision: u64 },
    ResourceAvailable,
    ControlGranted(p::ControlGranted),
    ControlRevoked(p::ControlRevoked),
    CommandResult(p::CommandResult),
    ResumeAccepted(p::ResumeAccepted),
    ResetRequired(p::ResetRequired),
    Pong(u64),
    Error(String),
}
pub struct NetworkClient {
    socket: WebSocketStream<websocket::ResponseGuard<tokio_rustls::client::TlsStream<TcpStream>>>,
    pub session_id: Id,
    pub epoch: Id,
    pub view_id: Id,
    pub limits: ReceiverLimits,
    pub profiles: Vec<String>,
    context: ResourceContext,
    receiver: ViewReceiver,
    needed: Option<NeededResources>,
    deadline: Duration,
}
impl NetworkClient {
    pub async fn connect(
        address: SocketAddr,
        server_name: String,
        pinned_certificate: Vec<u8>,
        credential: &Credential,
        mut hello: ClientHello,
    ) -> Result<Self, TransportError> {
        if hello.resume.is_some() {
            return Err(TransportError::Protocol);
        }
        let deadline = Duration::from_secs(5);
        let mut ceiling = hello.receiver_limits.envelope_bytes.min(1024 * 1024);
        if ceiling < websocket::MIN_MESSAGE {
            return Err(TransportError::Limit);
        }
        let mut connected = None;
        for _ in 0..2 {
            let stream = timeout(
                deadline,
                tls::connect_pinned(address, server_name.clone(), pinned_certificate.clone()),
            )
            .await
            .map_err(|_| TransportError::Deadline)??;
            let (socket, selected) = timeout(
                deadline,
                websocket::client_upgrade(stream, address, &server_name, ceiling),
            )
            .await
            .map_err(|_| TransportError::Deadline)??;
            if selected == ceiling {
                connected = Some(socket);
                break;
            }
            drop(socket);
            ceiling = selected;
        }
        let mut socket = connected.ok_or(TransportError::Protocol)?;
        hello.receiver_limits.envelope_bytes = ceiling;
        let mut wire = p::ClientHello {
            credential: credential.expose_for_tls_authentication().to_vec(),
            client_instance_id: hello.client_instance_id.to_vec(),
            supported_versions: hello.supported_versions.iter().copied().collect(),
            supported_profiles: hello.supported_profiles.iter().cloned().collect(),
            required_profiles: hello.required_profiles.iter().cloned().collect(),
            receiver_limits: Some(codec::wire_limits(&hello.receiver_limits)),
            resume: hello.resume.map(|r| p::ResumeRequest {
                epoch: r.epoch.to_vec(),
                view_id: r.view_id.to_vec(),
                acknowledged_revision: r.acknowledged_revision,
            }),
        };
        websocket::send(
            &mut socket,
            codec::wrap(p::Kind::ClientHello, &wire, None),
            ceiling,
            deadline,
        )
        .await?;
        wire.credential.zeroize();
        let raw = timeout(deadline, websocket::receive(&mut socket))
            .await
            .map_err(|_| TransportError::Deadline)??;
        let initial: p::Packet = codec::decode(&raw, ceiling)?;
        let binding = (
            codec::id(&initial.session_id)?,
            codec::id(&initial.epoch)?,
            codec::id(&initial.view_id)?,
        );
        let packet = codec::packet(&raw, ceiling, Some(binding))?;
        if packet.kind != p::Kind::ServerHello as i32 {
            return Err(TransportError::Protocol);
        }
        let server: p::ServerHello = codec::decode(&packet.payload, ceiling)?;
        let limits = codec::limits(server.negotiated_limits.ok_or(TransportError::Protocol)?)?;
        if limits.envelope_bytes != ceiling
            || server.selected_version != 1
            || hello
                .receiver_limits
                .negotiate(&limits)
                .map_err(|_| TransportError::Limit)?
                != limits
        {
            return Err(TransportError::Limit);
        }
        if !hello.supported_versions.contains(&server.selected_version)
            || server
                .selected_profiles
                .iter()
                .any(|p| !hello.supported_profiles.contains(p))
            || hello
                .required_profiles
                .iter()
                .any(|p| !server.selected_profiles.contains(p))
        {
            return Err(TransportError::Protocol);
        }
        rui_protocol_core::wire::required_features(&server.selected_profiles)
            .map_err(|_| TransportError::Protocol)?;
        let context = ResourceContext {
            scope: ResourceScope {
                session_id: binding.0,
                epoch: binding.1,
                principal_id: codec::id(&server.principal_id)?,
            },
            view_id: binding.2,
        };
        let config = ReceiverConfig {
            single_glyph_outlines: server
                .selected_profiles
                .iter()
                .any(|f| f == "glyph.outline-single.v1"),
            ..ReceiverConfig::default()
        };
        let receiver = ViewReceiver::new(context, limits.clone(), config)
            .map_err(|_| TransportError::Receiver)?;
        Ok(Self {
            socket,
            session_id: binding.0,
            epoch: binding.1,
            view_id: binding.2,
            limits,
            profiles: server.selected_profiles,
            context,
            receiver,
            needed: None,
            deadline,
        })
    }
    fn binding(&self) -> Option<(Id, Id, Id)> {
        Some((self.session_id, self.epoch, self.view_id))
    }
    pub fn receiver(&self) -> &ViewReceiver {
        &self.receiver
    }
    /// Low-level binary binding used by adapters and conformance tests. The host
    /// validates direction, kind, binding and authority for every packet.
    pub async fn send_packet(&mut self, packet: p::Packet) -> Result<(), TransportError> {
        websocket::send(
            &mut self.socket,
            packet,
            self.limits.envelope_bytes,
            self.deadline,
        )
        .await
    }
    pub async fn send<T: Message>(
        &mut self,
        kind: p::Kind,
        payload: &T,
    ) -> Result<(), TransportError> {
        self.send_packet(codec::wrap(kind, payload, self.binding()))
            .await
    }
    pub async fn open_view(&mut self) -> Result<(), TransportError> {
        self.send(
            p::Kind::OpenView,
            &p::OpenView {
                selection: "fixture".into(),
                mirror_canvas_policy: if self.profiles.iter().any(|p| p == "semantics.only.v1") {
                    "semantic-only"
                } else {
                    "host-canvas"
                }
                .into(),
            },
        )
        .await
    }
    pub async fn acquire_control(&mut self, request: Id) -> Result<(), TransportError> {
        self.send(
            p::Kind::AcquireControl,
            &p::AcquireControl {
                request_id: request.to_vec(),
            },
        )
        .await
    }
    pub async fn submit(&mut self, exact_command_bytes: &[u8]) -> Result<(), TransportError> {
        if exact_command_bytes.len() > self.limits.envelope_bytes.saturating_sub(128) {
            return Err(TransportError::Limit);
        }
        self.send_packet(codec::wrap_bytes(
            p::Kind::SemanticCommand,
            exact_command_bytes.to_vec(),
            self.binding(),
        ))
        .await
    }
    /// Trusted renderer completion callback; this is never called automatically
    /// merely because application state was received.
    pub async fn presented(&mut self, revision: u64) -> Result<(), TransportError> {
        if self.profiles.iter().any(|p| p == "semantics.only.v1") {
            return Err(TransportError::Authority);
        }
        self.receiver
            .mark_presented(revision)
            .map_err(|_| TransportError::Receiver)?;
        self.send(p::Kind::PresentedAck, &p::RevisionAck { revision })
            .await
    }
    /// Semantic-only consumption is explicit and never calls mark_presented.
    pub async fn semantic_ready(&mut self, revision: u64) -> Result<(), TransportError> {
        if !self.profiles.iter().any(|p| p == "semantics.only.v1")
            || self
                .receiver
                .state()
                .map_err(|_| TransportError::Receiver)?
                .revision
                != revision
            || !self
                .receiver
                .resources_ready()
                .map_err(|_| TransportError::Receiver)?
        {
            return Err(TransportError::Authority);
        }
        self.send(p::Kind::SemanticReady, &p::RevisionAck { revision })
            .await
    }
    pub async fn close(mut self) -> Result<(), TransportError> {
        self.send(
            p::Kind::CloseView,
            &p::Error {
                code: String::new(),
            },
        )
        .await
    }
    async fn outcome(&mut self, outcome: ReceiveOutcome) -> Result<ClientEvent, TransportError> {
        match outcome {
            ReceiveOutcome::Applied { revision } | ReceiveOutcome::Duplicate { revision } => {
                self.needed = None;
                self.send(p::Kind::AppliedAck, &p::RevisionAck { revision })
                    .await?;
                Ok(ClientEvent::Applied { revision })
            }
            ReceiveOutcome::NeedResources {
                revision, missing, ..
            } => {
                let maximum = self.limits.envelope_bytes.saturating_sub(256) / 40;
                if missing.len() > maximum {
                    return Err(TransportError::Limit);
                }
                let request = p::ResourceIds {
                    digests: missing.iter().map(|d| d.to_vec()).collect(),
                };
                self.needed = Some(missing);
                self.send(p::Kind::NeedResources, &request).await?;
                Ok(ClientEvent::ResourcesNeeded { revision })
            }
        }
    }
    pub async fn receive(&mut self) -> Result<ClientEvent, TransportError> {
        loop {
            let raw = timeout(self.deadline, websocket::receive(&mut self.socket))
                .await
                .map_err(|_| TransportError::Deadline)??;
            let packet = codec::packet(&raw, self.limits.envelope_bytes, self.binding())?;
            let kind = p::Kind::try_from(packet.kind).map_err(|_| TransportError::Protocol)?;
            let ceiling = self.limits.envelope_bytes;
            let context = self.context;
            match kind {
                p::Kind::ViewOpened => {
                    return Ok(ClientEvent::ViewOpened(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
                }
                p::Kind::SnapshotBegin | p::Kind::TransactionBegin => {
                    let wire: p::PublicationBegin = codec::decode(&packet.payload, ceiling)?;
                    let begin = codec::publication_begin(kind, wire)?;
                    self.receiver
                        .begin_publication(context, begin)
                        .map_err(|_| TransportError::Receiver)?;
                }
                p::Kind::PublicationChunk => {
                    let chunk: p::PublicationChunk = codec::decode(&packet.payload, ceiling)?;
                    if chunk.data.len() > self.limits.chunk_bytes {
                        return Err(TransportError::Limit);
                    }
                    self.receiver
                        .publication_chunk(
                            context,
                            codec::id(&chunk.publication_id)?,
                            chunk.index,
                            &chunk.data,
                        )
                        .map_err(|_| TransportError::Receiver)?;
                }
                p::Kind::PublicationCommit => {
                    let commit: p::PublicationCommit = codec::decode(&packet.payload, ceiling)?;
                    let id = codec::id(&commit.publication_id)?;
                    let outcome = self
                        .receiver
                        .commit_publication(context, id, codec::publication_commit(commit)?)
                        .map_err(|_| TransportError::Receiver)?;
                    return self.outcome(outcome).await;
                }
                p::Kind::AbortPublication => {
                    let abort: p::AbortPublication = codec::decode(&packet.payload, ceiling)?;
                    self.receiver
                        .abort_publication(context, codec::id(&abort.publication_id)?)
                        .map_err(|_| TransportError::Receiver)?;
                }
                p::Kind::ResourceOffer => {
                    let offer = codec::resource_offer(codec::decode(&packet.payload, ceiling)?)?;
                    if !self
                        .needed
                        .as_ref()
                        .is_some_and(|needed| needed.contains(&offer.digest))
                    {
                        return Err(TransportError::Authentication);
                    }
                    self.receiver
                        .authorize_resource(offer.digest)
                        .map_err(|_| TransportError::Receiver)?;
                    self.receiver
                        .receive_resource_offer(context, offer)
                        .map_err(|_| TransportError::Receiver)?;
                }
                p::Kind::ResourceChunk => {
                    let chunk: p::ResourceChunk = codec::decode(&packet.payload, ceiling)?;
                    if chunk.data.len() > self.limits.chunk_bytes {
                        return Err(TransportError::Limit);
                    }
                    self.receiver
                        .resource_chunk(
                            context,
                            codec::digest(&chunk.digest)?,
                            chunk.index,
                            &chunk.data,
                        )
                        .map_err(|_| TransportError::Receiver)?;
                }
                p::Kind::ResourceComplete => {
                    let complete: p::ResourceComplete = codec::decode(&packet.payload, ceiling)?;
                    let digest = codec::digest(&complete.digest)?;
                    let outcome = self
                        .receiver
                        .complete_resource(context, digest)
                        .map_err(|_| TransportError::Receiver)?;
                    self.send(
                        p::Kind::ResourceAvailable,
                        &p::ResourceIds {
                            digests: vec![digest.to_vec()],
                        },
                    )
                    .await?;
                    if let Some(outcome) = outcome {
                        return self.outcome(outcome).await;
                    }
                    return Ok(ClientEvent::ResourceAvailable);
                }
                p::Kind::ControlGranted => {
                    return Ok(ClientEvent::ControlGranted(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
                }
                p::Kind::ControlRevoked => {
                    return Ok(ClientEvent::ControlRevoked(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
                }
                p::Kind::CommandResult => {
                    return Ok(ClientEvent::CommandResult(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
                }
                p::Kind::ResumeAccepted => {
                    return Ok(ClientEvent::ResumeAccepted(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
                }
                p::Kind::ResetRequired => {
                    return Ok(ClientEvent::ResetRequired(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
                }
                p::Kind::Error => {
                    let error: p::Error = codec::decode(&packet.payload, ceiling)?;
                    return Ok(ClientEvent::Error(error.code));
                }
                p::Kind::Pong => {
                    let pong: p::Ping = codec::decode(&packet.payload, ceiling)?;
                    return Ok(ClientEvent::Pong(pong.nonce));
                }
                _ => return Err(TransportError::Protocol),
            }
        }
    }
}
