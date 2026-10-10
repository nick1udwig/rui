//! Bounded session actor; caller supplies authenticated ordered bytes, clock and presentation.
use crate::{command_queue::CommandQueue, interaction, wire_command::wire_command, Viewport};
use prost::Message;
use rui_protocol_core::{
    commands::{Arguments, CommandFamily, Id},
    model::{Point, Record, State},
    receiver::{NeededResources, ReceiveOutcome, ReceiverConfig, ViewReceiver},
    resources::{MemoryReservation, ResourceContext, ResourceScope},
    session::ReceiverLimits,
};
use rui_wire::{codec, proto as p, WireError as TransportError};
use serde::Serialize;
use std::collections::VecDeque;
const PROFILES: &[&str] = &[
    "mirror2d.core.v1",
    "semantics.core.v1",
    "content.no-raster.v1",
    "glyph.outline-single.v1",
    "text.clock.gtk-editable4224.v1",
];
const REQUIRED: &[&str] = &[
    "mirror2d.core.v1",
    "semantics.core.v1",
    "content.no-raster.v1",
    "glyph.outline-single.v1",
];
const SEMANTIC_PROFILES: &[&str] = &[
    "mirror2d.core.v1",
    "semantics.core.v1",
    "content.no-raster.v1",
    "semantics.only.v1",
    "text.clock.gtk-editable4224.v1",
];
const SEMANTIC_REQUIRED: &[&str] = &[
    "mirror2d.core.v1",
    "semantics.core.v1",
    "content.no-raster.v1",
    "semantics.only.v1",
];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PresentationMode {
    Mirror,
    SemanticOnly,
}
#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub mode: PresentationMode,
    pub profiles: Vec<String>,
    pub can_control: bool,
    pub actions: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Connected {
        can_control: bool,
    },
    ViewOpened {
        width: f64,
        height: f64,
    },
    Applied {
        revision: u64,
    },
    ResourcesNeeded {
        revision: u64,
    },
    Control {
        granted: bool,
    },
    CommandResult {
        sequence: u64,
        status: String,
        rejection: String,
        source_commit: String,
        revision: Option<u64>,
    },
    Resumed,
    Reset,
    Error {
        code: String,
    },
    None,
}
struct Outbound {
    bytes: Vec<u8>,
    _memory: Option<MemoryReservation>,
}
/// Retains committed state and exact pending commands across socket replacement.
/// Credentials are only copied into the outgoing handshake, never retained here.
pub struct Session {
    client: Id,
    mode: PresentationMode,
    allowed_actions: Vec<String>,
    selection: String,
    limits: ReceiverLimits,
    context: Option<ResourceContext>,
    receiver: Option<ViewReceiver>,
    commands: Option<CommandQueue>,
    missing: Option<NeededResources>,
    outbound: VecDeque<Outbound>,
    outbound_bytes: usize,
    connected: bool,
    starting: bool,
    awaiting_resume: Option<u64>,
    can_control: bool,
    lease: Option<p::ControlGranted>,
    request: Option<Id>,
    cancelled_request: Option<Id>,
    retained_limits: Option<ReceiverLimits>,
    serial: u64,
    renewed_at: u64,
    profiles: Vec<String>,
    ready: Option<u64>,
}
impl Session {
    pub fn new(client: Id, selection: String) -> Result<Self, TransportError> {
        Self::with_mode(client, selection, PresentationMode::Mirror)
    }
    pub fn with_mode(
        client: Id,
        selection: String,
        mode: PresentationMode,
    ) -> Result<Self, TransportError> {
        if client == [0; 16] || selection.is_empty() || selection.len() > 256 {
            return Err(TransportError::Protocol);
        }
        Ok(Self {
            client,
            mode,
            allowed_actions: Vec::new(),
            selection,
            limits: ReceiverLimits::default(),
            context: None,
            receiver: None,
            commands: None,
            missing: None,
            outbound: VecDeque::new(),
            outbound_bytes: 0,
            connected: false,
            starting: false,
            awaiting_resume: None,
            can_control: false,
            lease: None,
            request: None,
            cancelled_request: None,
            retained_limits: None,
            serial: 1,
            renewed_at: 0,
            profiles: Vec::new(),
            ready: None,
        })
    }
    fn supported_profiles(&self) -> &'static [&'static str] {
        match self.mode {
            PresentationMode::Mirror => PROFILES,
            PresentationMode::SemanticOnly => SEMANTIC_PROFILES,
        }
    }
    fn required_profiles(&self) -> &'static [&'static str] {
        match self.mode {
            PresentationMode::Mirror => REQUIRED,
            PresentationMode::SemanticOnly => SEMANTIC_REQUIRED,
        }
    }
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            mode: self.mode,
            profiles: self.profiles.clone(),
            can_control: self.can_control,
            actions: self.allowed_actions.clone(),
        }
    }
    pub fn start(&mut self, credential: &[u8]) -> Result<(), TransportError> {
        if self.connected || self.starting || credential.len() != 32 {
            return Err(TransportError::Protocol);
        }
        self.limits = ReceiverLimits::default();
        let resume = self.context.zip(self.receiver.as_ref()).map(|(c, r)| {
            let revision = r.state().map(|s| s.revision).unwrap_or(0);
            self.awaiting_resume = Some(revision);
            p::ResumeRequest {
                epoch: c.scope.epoch.to_vec(),
                view_id: c.view_id.to_vec(),
                acknowledged_revision: revision,
            }
        });
        let hello = p::ClientHello {
            credential: credential.to_vec(),
            client_instance_id: self.client.to_vec(),
            supported_versions: vec![1],
            supported_profiles: self
                .supported_profiles()
                .iter()
                .map(|s| (*s).into())
                .collect(),
            required_profiles: self
                .required_profiles()
                .iter()
                .map(|s| (*s).into())
                .collect(),
            receiver_limits: Some(codec::wire_limits(&self.limits)),
            resume,
            fresh_view: false,
        };
        self.starting = true;
        self.enqueue(codec::wrap(p::Kind::ClientHello, &hello, None))
    }
    fn binding(&self) -> Option<(Id, Id, Id)> {
        self.context
            .map(|c| (c.scope.session_id, c.scope.epoch, c.view_id))
    }
    fn enqueue(&mut self, packet: p::Packet) -> Result<(), TransportError> {
        let length = packet.encoded_len();
        if length > self.limits.envelope_bytes
            || self.outbound.len() >= 32
            || self.outbound_bytes.saturating_add(length) > self.limits.outbound_queue_bytes
        {
            return Err(TransportError::Limit);
        }
        let memory = self
            .receiver
            .as_ref()
            .map(|r| {
                r.memory_budget()
                    .reserve(length.saturating_mul(4).saturating_add(4096))
                    .map_err(|_| TransportError::Limit)
            })
            .transpose()?;
        self.outbound_bytes += length;
        self.outbound.push_back(Outbound {
            bytes: packet.encode_to_vec(),
            _memory: memory,
        });
        Ok(())
    }
    fn send<T: Message>(&mut self, kind: p::Kind, message: &T) -> Result<(), TransportError> {
        if !self.connected {
            return Err(TransportError::Closed);
        }
        self.enqueue(codec::wrap(kind, message, self.binding()))
    }
    /// Check transport capacity before removing a frame. Commands remain in the
    /// exact-byte ledger if send subsequently fails with an ambiguous outcome.
    pub fn next_outbound(&mut self) -> Option<Vec<u8>> {
        let f = self.outbound.pop_front()?;
        self.outbound_bytes -= f.bytes.len();
        Some(f.bytes)
    }
    pub fn ceiling(&self) -> usize {
        self.limits.envelope_bytes
    }
    pub fn memory_used(&self) -> usize {
        self.receiver.as_ref().map_or(0, ViewReceiver::memory_used)
    }
    pub fn receiver(&self) -> Result<&ViewReceiver, TransportError> {
        self.receiver.as_ref().ok_or(TransportError::Closed)
    }
    pub fn state(&self) -> Result<&State, TransportError> {
        self.receiver()?
            .state()
            .map_err(|_| TransportError::Receiver)
    }
    pub fn has_control(&self) -> bool {
        self.connected
            && self.lease.is_some()
            && self.ready == self.state().ok().map(|s| s.revision)
    }
    pub fn pending_commands(&self) -> usize {
        self.commands.as_ref().map_or(0, CommandQueue::len)
    }
    fn outcome(&mut self, outcome: ReceiveOutcome) -> Result<Event, TransportError> {
        match outcome {
            ReceiveOutcome::Applied { revision } | ReceiveOutcome::Duplicate { revision } => {
                self.missing = None;
                self.ready = None;
                self.send(p::Kind::AppliedAck, &p::RevisionAck { revision })?;
                Ok(Event::Applied { revision })
            }
            ReceiveOutcome::NeedResources {
                revision, missing, ..
            } => {
                self.ready = None;
                if missing.len() > 512 {
                    return Err(TransportError::Limit);
                }
                let page = (self.limits.envelope_bytes.saturating_sub(256) / 40).max(1);
                let digests: Vec<_> = missing.iter().map(|d| d.to_vec()).collect();
                for chunk in digests.chunks(page) {
                    self.send(
                        p::Kind::NeedResources,
                        &p::ResourceIds {
                            digests: chunk.to_vec(),
                        },
                    )?;
                }
                self.missing = Some(missing);
                Ok(Event::ResourcesNeeded { revision })
            }
        }
    }
    fn handshake(&mut self, raw: &[u8]) -> Result<Event, TransportError> {
        let initial: p::Packet = codec::decode(raw, self.limits.envelope_bytes)?;
        if initial.kind == p::Kind::Error as i32 {
            let e: p::Error = codec::decode(&initial.payload, self.limits.envelope_bytes)?;
            return Ok(Event::Error { code: e.code });
        }
        let binding = (
            codec::id(&initial.session_id)?,
            codec::id(&initial.epoch)?,
            codec::id(&initial.view_id)?,
        );
        let packet = codec::packet(raw, self.limits.envelope_bytes, Some(binding))?;
        if packet.kind != p::Kind::ServerHello as i32 {
            return Err(TransportError::Protocol);
        }
        let hello: p::ServerHello = codec::decode(&packet.payload, self.limits.envelope_bytes)?;
        if hello.selected_version != 1
            || hello
                .selected_profiles
                .iter()
                .any(|s| !self.supported_profiles().contains(&s.as_str()))
            || self
                .required_profiles()
                .iter()
                .any(|s| !hello.selected_profiles.iter().any(|p| p == s))
        {
            return Err(TransportError::Protocol);
        }
        let limits = codec::limits(hello.negotiated_limits.ok_or(TransportError::Protocol)?)?;
        if self
            .limits
            .negotiate(&limits)
            .map_err(|_| TransportError::Limit)?
            != limits
        {
            return Err(TransportError::Limit);
        }
        let permissions = hello.permissions.ok_or(TransportError::Protocol)?;
        if !permissions.can_view {
            return Err(TransportError::Authority);
        }
        let context = ResourceContext {
            scope: ResourceScope {
                session_id: binding.0,
                epoch: binding.1,
                principal_id: codec::id(&hello.principal_id)?,
            },
            view_id: binding.2,
        };
        if self.context == Some(context) {
            if self.profiles != hello.selected_profiles
                || self.retained_limits.as_ref() != Some(&limits)
            {
                return Err(TransportError::Protocol);
            }
            self.receiver
                .as_mut()
                .ok_or(TransportError::Receiver)?
                .reconnect(context)
                .map_err(|_| TransportError::Receiver)?;
        } else {
            let receiver = ViewReceiver::new(
                context,
                limits.clone(),
                ReceiverConfig {
                    single_glyph_outlines: hello
                        .selected_profiles
                        .iter()
                        .any(|p| p == "glyph.outline-single.v1"),
                    ..Default::default()
                },
            )
            .map_err(|_| TransportError::Receiver)?;
            if let Some(q) = &mut self.commands {
                q.rebind(
                    context.scope.epoch,
                    receiver.memory_budget(),
                    limits.envelope_bytes.saturating_sub(256),
                )
                .map_err(|_| TransportError::Limit)?;
            } else {
                self.commands = Some(
                    CommandQueue::new(
                        context.scope.epoch,
                        receiver.memory_budget(),
                        limits.envelope_bytes.saturating_sub(256),
                    )
                    .map_err(|_| TransportError::Limit)?,
                );
            }
            self.receiver = Some(receiver);
        }
        self.context = Some(context);
        self.retained_limits = Some(limits.clone());
        self.limits = limits;
        self.profiles = hello.selected_profiles;
        self.can_control = permissions.can_control;
        self.allowed_actions = permissions.actions;
        self.connected = true;
        self.starting = false;
        self.send(
            p::Kind::OpenView,
            &p::OpenView {
                selection: self.selection.clone(),
                mirror_canvas_policy: match self.mode {
                    PresentationMode::Mirror => "host-canvas",
                    PresentationMode::SemanticOnly => "semantic-only",
                }
                .into(),
            },
        )?;
        let pending: Vec<_> = self
            .commands
            .as_ref()
            .unwrap()
            .commands()
            .map(|c| c.sequence())
            .collect();
        for command_seq in pending {
            self.send(
                p::Kind::GetCommandResult,
                &p::GetCommandResult { command_seq },
            )?;
        }
        Ok(Event::Connected {
            can_control: self.can_control,
        })
    }
    pub fn receive(&mut self, raw: &[u8]) -> Result<Event, TransportError> {
        let result = self.receive_inner(raw);
        if result.is_err() {
            self.disconnect();
        }
        result
    }
    fn receive_inner(&mut self, raw: &[u8]) -> Result<Event, TransportError> {
        if self.starting {
            return self.handshake(raw);
        }
        if !self.connected {
            return Err(TransportError::Closed);
        }
        let _decode = self
            .receiver()?
            .memory_budget()
            .reserve(raw.len().saturating_mul(4).saturating_add(8192))
            .map_err(|_| TransportError::Limit)?;
        let packet = codec::packet(raw, self.limits.envelope_bytes, self.binding())?;
        let kind = p::Kind::try_from(packet.kind).map_err(|_| TransportError::Protocol)?;
        let ceiling = self.limits.envelope_bytes;
        let context = self.context.ok_or(TransportError::Binding)?;
        let receiver = self.receiver.as_mut().ok_or(TransportError::Closed)?;
        match kind {
            p::Kind::ViewOpened => {
                let v = codec::view_opened(&packet.payload, ceiling)?;
                if codec::id(&v.view_id)? != context.view_id {
                    return Err(TransportError::Binding);
                }
                Ok(Event::ViewOpened {
                    width: v.canvas_width,
                    height: v.canvas_height,
                })
            }
            p::Kind::SnapshotBegin | p::Kind::TransactionBegin => {
                let b = codec::publication_begin(kind, codec::decode(&packet.payload, ceiling)?)?;
                receiver
                    .begin_publication(context, b)
                    .map_err(|_| TransportError::Receiver)?;
                Ok(Event::None)
            }
            p::Kind::PublicationChunk => {
                let c: p::PublicationChunk = codec::decode(&packet.payload, ceiling)?;
                if c.data.len() > self.limits.chunk_bytes {
                    return Err(TransportError::Limit);
                }
                receiver
                    .publication_chunk(context, codec::id(&c.publication_id)?, c.index, &c.data)
                    .map_err(|_| TransportError::Receiver)?;
                Ok(Event::None)
            }
            p::Kind::PublicationCommit => {
                let c: p::PublicationCommit = codec::decode(&packet.payload, ceiling)?;
                let outcome = receiver
                    .commit_authenticated_publication(
                        context,
                        codec::id(&c.publication_id)?,
                        codec::publication_commit(c)?,
                    )
                    .map_err(|_| TransportError::Receiver)?;
                self.outcome(outcome)
            }
            p::Kind::AbortPublication => {
                let a: p::AbortPublication = codec::decode(&packet.payload, ceiling)?;
                receiver
                    .abort_publication(context, codec::id(&a.publication_id)?)
                    .map_err(|_| TransportError::Receiver)?;
                Ok(Event::None)
            }
            p::Kind::ResourceOffer => {
                let o = codec::resource_offer(codec::decode(&packet.payload, ceiling)?)?;
                if !self.missing.as_ref().is_some_and(|m| m.contains(&o.digest)) {
                    return Err(TransportError::Authentication);
                }
                receiver
                    .authorize_resource(o.digest)
                    .map_err(|_| TransportError::Receiver)?;
                receiver
                    .receive_resource_offer(context, o)
                    .map_err(|_| TransportError::Receiver)?;
                Ok(Event::None)
            }
            p::Kind::ResourceChunk => {
                let c: p::ResourceChunk = codec::decode(&packet.payload, ceiling)?;
                if c.data.len() > self.limits.chunk_bytes {
                    return Err(TransportError::Limit);
                }
                receiver
                    .resource_chunk(context, codec::digest(&c.digest)?, c.index, &c.data)
                    .map_err(|_| TransportError::Receiver)?;
                Ok(Event::None)
            }
            p::Kind::ResourceComplete => {
                let c: p::ResourceComplete = codec::decode(&packet.payload, ceiling)?;
                let digest = codec::digest(&c.digest)?;
                let outcome = receiver
                    .complete_resource(context, digest)
                    .map_err(|_| TransportError::Receiver)?;
                self.send(
                    p::Kind::ResourceAvailable,
                    &p::ResourceIds {
                        digests: vec![digest.to_vec()],
                    },
                )?;
                match outcome {
                    Some(ReceiveOutcome::NeedResources { .. }) | None => Ok(Event::None),
                    Some(o) => self.outcome(o),
                }
            }
            p::Kind::ResumeAccepted => {
                let r: p::ResumeAccepted = codec::decode(&packet.payload, ceiling)?;
                let revision = receiver
                    .state()
                    .map_err(|_| TransportError::Receiver)?
                    .revision;
                if self.awaiting_resume.take() != Some(r.from_revision)
                    || revision != r.from_revision
                    || r.through_revision < revision
                {
                    return Err(TransportError::Protocol);
                }
                if r.through_revision == revision {
                    self.outcome(ReceiveOutcome::Applied { revision })?;
                }
                Ok(Event::Resumed)
            }
            p::Kind::ResetRequired => {
                let _: p::ResetRequired = codec::decode(&packet.payload, ceiling)?;
                if self.awaiting_resume.take().is_none() {
                    return Err(TransportError::Protocol);
                }
                self.ready = None;
                Ok(Event::Reset)
            }
            p::Kind::ControlGranted => {
                let g: p::ControlGranted = codec::decode(&packet.payload, ceiling)?;
                let request = codec::id(&g.request_id)?;
                if self.cancelled_request == Some(request) {
                    self.cancelled_request = None;
                    let release_id = self.request_id()?;
                    self.send(
                        p::Kind::ReleaseControl,
                        &p::ReleaseControl {
                            request_id: release_id.to_vec(),
                            lease_id: g.lease_id,
                            generation: g.generation,
                        },
                    )?;
                    return Ok(Event::None);
                }
                if self.request.take() != Some(request)
                    || !self.can_control
                    || codec::id(&g.lease_id)? == [0; 16]
                    || g.generation == 0
                {
                    return Err(TransportError::Authority);
                }
                self.lease = Some(g);
                Ok(Event::Control { granted: true })
            }
            p::Kind::ControlRevoked => {
                let r: p::ControlRevoked = codec::decode(&packet.payload, ceiling)?;
                if self
                    .lease
                    .as_ref()
                    .is_some_and(|g| g.lease_id == r.lease_id && g.generation == r.generation)
                {
                    self.lease = None;
                }
                Ok(Event::Control {
                    granted: self.lease.is_some(),
                })
            }
            p::Kind::CommandResult => {
                let r: p::CommandResult = codec::decode(&packet.payload, ceiling)?;
                if !matches!(
                    r.status.to_ascii_lowercase().as_str(),
                    "dispatched" | "unknown" | "outcomeunknown" | "outcomeexpired"
                ) {
                    self.commands.as_mut().unwrap().complete(r.command_seq);
                }
                Ok(Event::CommandResult {
                    sequence: r.command_seq,
                    status: r.status,
                    rejection: r.rejection,
                    source_commit: r.source_commit,
                    revision: r.resulting_view_revision,
                })
            }
            p::Kind::Error => {
                let e: p::Error = codec::decode(&packet.payload, ceiling)?;
                self.cancelled_request = self.request.take();
                self.lease = None;
                Ok(Event::Error { code: e.code })
            }
            p::Kind::Pong => {
                let _: p::Ping = codec::decode(&packet.payload, ceiling)?;
                Ok(Event::None)
            }
            _ => Err(TransportError::Protocol),
        }
    }
    pub fn presented(&mut self, revision: u64) -> Result<(), TransportError> {
        if self.mode != PresentationMode::Mirror {
            return Err(TransportError::Authority);
        }
        if !self.connected || self.state()?.revision != revision {
            return Err(TransportError::Receiver);
        }
        self.receiver
            .as_mut()
            .unwrap()
            .mark_presented(revision)
            .map_err(|_| TransportError::Receiver)?;
        self.send(p::Kind::PresentedAck, &p::RevisionAck { revision })?;
        self.ready = Some(revision);
        Ok(())
    }
    /// A semantic consumer explicitly accepted this exact validated revision.
    /// This never marks visual presentation and cannot authorize other families.
    pub fn semantic_ready(&mut self, revision: u64) -> Result<(), TransportError> {
        if self.mode != PresentationMode::SemanticOnly
            || !self.connected
            || self.state()?.revision != revision
            || !self
                .receiver()?
                .resources_ready()
                .map_err(|_| TransportError::Receiver)?
        {
            return Err(TransportError::Authority);
        }
        self.send(p::Kind::SemanticReady, &p::RevisionAck { revision })?;
        self.ready = Some(revision);
        Ok(())
    }
    fn request_id(&mut self) -> Result<Id, TransportError> {
        let mut id = self.client;
        id[8..].copy_from_slice(&self.serial.to_be_bytes());
        self.serial = self.serial.checked_add(1).ok_or(TransportError::Limit)?;
        Ok(id)
    }
    pub fn acquire(&mut self, now: u64) -> Result<(), TransportError> {
        if !self.can_control || self.ready != Some(self.state()?.revision) || self.request.is_some()
        {
            return Err(TransportError::Authority);
        }
        let request_id = self.request_id()?;
        self.send(
            p::Kind::AcquireControl,
            &p::AcquireControl {
                request_id: request_id.to_vec(),
            },
        )?;
        self.request = Some(request_id);
        self.renewed_at = now;
        Ok(())
    }
    pub fn tick(&mut self, now: u64) -> Result<(), TransportError> {
        if !self.connected || now.saturating_sub(self.renewed_at) < 2000 || self.request.is_some() {
            return Ok(());
        }
        if let Some(g) = self.lease.clone() {
            let id = self.request_id()?;
            self.send(
                p::Kind::RenewControl,
                &p::RenewControl {
                    request_id: id.to_vec(),
                    lease_id: g.lease_id,
                    generation: g.generation,
                },
            )?;
            self.request = Some(id);
        } else {
            self.send(p::Kind::Ping, &p::Ping { nonce: now })?;
        }
        self.renewed_at = now;
        Ok(())
    }
    pub fn release(&mut self) -> Result<(), TransportError> {
        self.ready = None;
        self.cancelled_request = self.request.take();
        if let Some(g) = self.lease.take() {
            let id = self.request_id()?;
            self.send(
                p::Kind::ReleaseControl,
                &p::ReleaseControl {
                    request_id: id.to_vec(),
                    lease_id: g.lease_id,
                    generation: g.generation,
                },
            )?;
        }
        Ok(())
    }
    pub fn hit_test(&self, viewport: Viewport, point: Point) -> Result<u64, TransportError> {
        if self.mode != PresentationMode::Mirror {
            return Err(TransportError::Authority);
        }
        interaction::hit_test(
            self.state()?,
            self.ready.ok_or(TransportError::Authority)?,
            viewport,
            point,
        )
        .map(|t| t.semantic)
        .map_err(|_| TransportError::Authority)
    }
    pub fn command_binding(&self) -> Result<interaction::CommandBinding, TransportError> {
        let c = self.context.ok_or(TransportError::Binding)?;
        let g = self.lease.as_ref().ok_or(TransportError::Authority)?;
        Ok(interaction::CommandBinding {
            epoch: c.scope.epoch,
            view: c.view_id,
            client: self.client,
            lease: codec::id(&g.lease_id)?,
            generation: g.generation,
            sequence: self
                .commands
                .as_ref()
                .ok_or(TransportError::Closed)?
                .next_sequence(),
        })
    }
    /// Opaque capture stamp covers identity, authority, focus, selection and all
    /// source clocks. A draft cannot survive a release/regrant or context ABA.
    pub fn edit_token(&self, target: u64) -> Result<[u8; 32], TransportError> {
        if !self.has_control() {
            return Err(TransportError::Authority);
        }
        use sha2::{Digest, Sha256};
        let captured =
            interaction::editing_context(self.state()?, target, &self.command_binding()?)
                .ok_or(TransportError::Authority)?;
        let mut hash = Sha256::new();
        for id in [captured.epoch, captured.view, captured.lease] {
            hash.update(id);
        }
        for clock in [
            captured.generation,
            captured.revision,
            captured.target,
            captured.target_version,
            captured.text,
            captured.text_record_version,
            captured.text_version,
            captured.focus_generation,
            captured.selection.anchor,
            captured.selection.focus,
        ] {
            hash.update(clock.to_be_bytes());
        }
        Ok(hash.finalize().into())
    }
    pub fn command(
        &mut self,
        target: u64,
        action: &str,
        args: Arguments,
    ) -> Result<u64, TransportError> {
        if self.ready != Some(self.state()?.revision) || self.pending_commands() != 0 {
            return Err(TransportError::Authority);
        }
        if !self
            .lease
            .as_ref()
            .is_some_and(|g| g.allowed_actions.iter().any(|a| a == action))
        {
            return Err(TransportError::Authority);
        }
        let command =
            interaction::command(self.state()?, target, action, args, self.command_binding()?)
                .map_err(|_| TransportError::Authority)?;
        let wire = wire_command(command);
        let q = self.commands.as_mut().unwrap();
        let seq = q
            .encode(CommandFamily::Semantic, wire.encoded_len(), |b| {
                wire.encode(b).map_err(|_| ())
            })
            .map_err(|_| TransportError::Limit)?;
        let payload = q.latest().unwrap().bytes().to_vec();
        self.enqueue(codec::wrap_bytes(
            p::Kind::SemanticCommand,
            payload,
            self.binding(),
        ))?;
        Ok(seq)
    }
    pub fn activate_item(
        &mut self,
        collection: u64,
        item: u64,
        version: u64,
    ) -> Result<u64, TransportError> {
        let Some(Record::Semantic(owner)) = self.state()?.records.get(&collection) else {
            return Err(TransportError::Authority);
        };
        let model = owner.collection.as_ref().ok_or(TransportError::Authority)?;
        if model.version != version || !model.items.iter().any(|r| r.id == item) {
            return Err(TransportError::Authority);
        }
        self.command(item, "Activate", Arguments::None)
    }
    /// Explicit recovery resends only the original command bytes, including
    /// original epoch, lease, view and sequence. Never manufacture a fresh edit.
    pub fn retry_pending(&mut self) -> Result<(), TransportError> {
        if !self.connected {
            return Err(TransportError::Closed);
        }
        let q = self.commands.as_ref().ok_or(TransportError::Closed)?;
        let pending: Vec<_> = q.commands().map(|c| c.bytes().to_vec()).collect();
        for bytes in pending {
            self.enqueue(codec::wrap_bytes(
                p::Kind::SemanticCommand,
                bytes,
                self.binding(),
            ))?;
        }
        Ok(())
    }
    pub fn disconnect(&mut self) {
        self.connected = false;
        self.starting = false;
        self.can_control = false;
        self.allowed_actions.clear();
        self.lease = None;
        self.request = None;
        self.cancelled_request = None;
        self.ready = None;
        self.missing = None;
        for f in &mut self.outbound {
            f.bytes.fill(0);
        }
        self.outbound.clear();
        self.outbound_bytes = 0;
        if let Some(r) = &mut self.receiver {
            r.disconnect();
        }
    }
    pub fn close(&mut self) {
        self.disconnect();
        self.commands = None;
        if let Some(r) = &mut self.receiver {
            r.close();
        }
        self.receiver = None;
        self.context = None;
    }
    pub fn text(&self, target: u64) -> Option<&rui_protocol_core::model::TextModel> {
        let Record::Semantic(n) = self.state().ok()?.records.get(&target)? else {
            return None;
        };
        let Record::Text(t) = self.state().ok()?.records.get(&n.text?.id)? else {
            return None;
        };
        Some(t)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_protocol_core::{
        publication::publication_digest,
        resources::{resource_digest, ResourceKind, ResourceProvenance},
        wire::proto as pb,
    };
    fn bound<T: Message>(kind: p::Kind, payload: &T) -> Vec<u8> {
        codec::wrap(kind, payload, Some(([1; 16], [2; 16], [4; 16]))).encode_to_vec()
    }
    fn hello(epoch: u8, control: bool) -> Vec<u8> {
        codec::wrap(
            p::Kind::ServerHello,
            &p::ServerHello {
                selected_version: 1,
                selected_profiles: PROFILES.iter().map(|s| (*s).into()).collect(),
                negotiated_limits: Some(codec::wire_limits(&ReceiverLimits {
                    envelope_bytes: 128 << 10,
                    ..Default::default()
                })),
                permissions: Some(p::Permissions {
                    can_view: true,
                    can_control: control,
                    actions: vec!["Activate".into()],
                }),
                principal_id: vec![3; 16],
            },
            Some(([1; 16], [epoch; 16], [4; 16])),
        )
        .encode_to_vec()
    }
    fn session(control: bool) -> Session {
        let mut s = Session::new([3; 16], "fixture".into()).unwrap();
        s.start(&[7; 32]).unwrap();
        let raw = s.next_outbound().unwrap();
        let packet = codec::packet(&raw, s.ceiling(), None).unwrap();
        assert_eq!(packet.kind, p::Kind::ClientHello as i32);
        s.receive(&hello(2, control)).unwrap();
        drain(&mut s);
        s
    }
    fn drain(s: &mut Session) -> Vec<p::Packet> {
        let mut v = vec![];
        while let Some(raw) = s.next_outbound() {
            v.push(codec::decode(&raw, s.ceiling()).unwrap());
        }
        v
    }
    fn record(version: u64, digest: Option<[u8; 32]>) -> pb::Record {
        if let Some(digest) = digest {
            return pb::Record {
                record: Some(pb::record::Record::Scene(pb::SceneNode {
                    id: 2,
                    version: 1,
                    transform: Some(pb::Affine {
                        xx: 1.,
                        yy: 1.,
                        ..Default::default()
                    }),
                    opacity: 1.,
                    primitive: Some(pb::scene_node::Primitive::FillPath(pb::FillPath {
                        resource: digest.to_vec(),
                        color: Some(pb::Color {
                            alpha: 1.,
                            ..Default::default()
                        }),
                        fill_rule: 1,
                    })),
                    ..Default::default()
                })),
            };
        }
        pb::Record {
            record: Some(pb::record::Record::Semantic(pb::SemanticNode {
                id: 1,
                version,
                role: pb::SemanticRole::Button as i32,
                actions: vec![pb::Action {
                    id: "Activate".into(),
                    schema_version: 1,
                }],
                sensitivity: 1,
                completeness: 1,
                ..Default::default()
            })),
        }
    }
    fn publish(
        s: &mut Session,
        revision: u64,
        digest: Option<[u8; 32]>,
        transaction: bool,
    ) -> Event {
        let payload = if transaction {
            pb::Transaction {
                base_revision: revision - 1,
                new_revision: revision,
                mutations: vec![pb::Mutation {
                    mutation: Some(pb::mutation::Mutation::Put(record(revision, digest))),
                }],
                ..Default::default()
            }
            .encode_to_vec()
        } else {
            pb::Snapshot {
                revision,
                records: vec![record(revision, digest)],
                ..Default::default()
            }
            .encode_to_vec()
        };
        let bytes = payload;
        let hash = publication_digest(&bytes);
        let id = vec![revision as u8; 16];
        s.receive(&bound(
            if transaction {
                p::Kind::TransactionBegin
            } else {
                p::Kind::SnapshotBegin
            },
            &p::PublicationBegin {
                publication_id: id.clone(),
                base_revision: if transaction { revision - 1 } else { 0 },
                revision,
                total_bytes: bytes.len() as u64,
                chunk_count: 1,
                item_count: 1,
                digest: hash.to_vec(),
            },
        ))
        .unwrap();
        s.receive(&bound(
            p::Kind::PublicationChunk,
            &p::PublicationChunk {
                publication_id: id.clone(),
                index: 0,
                data: bytes.clone(),
            },
        ))
        .unwrap();
        s.receive(&bound(
            p::Kind::PublicationCommit,
            &p::PublicationCommit {
                publication_id: id,
                total_bytes: bytes.len() as u64,
                chunk_count: 1,
                item_count: 1,
                digest: hash.to_vec(),
            },
        ))
        .unwrap()
    }
    fn grant(s: &mut Session) -> p::ControlGranted {
        s.acquire(100).unwrap();
        let frames = drain(s);
        let a: p::AcquireControl =
            codec::decode(&frames.last().unwrap().payload, s.ceiling()).unwrap();
        let grant = p::ControlGranted {
            lease_id: vec![8; 16],
            generation: 1,
            expires_at_ms: 5100,
            allowed_actions: vec!["Activate".into()],
            request_id: a.request_id,
        };
        s.receive(&bound(p::Kind::ControlGranted, &grant)).unwrap();
        grant
    }
    #[test]
    fn semantic_mode_negotiates_existing_override_and_never_presents() {
        let mut s =
            Session::with_mode([3; 16], "owned".into(), PresentationMode::SemanticOnly).unwrap();
        s.start(&[7; 32]).unwrap();
        let packet: p::Packet = codec::decode(&s.next_outbound().unwrap(), s.ceiling()).unwrap();
        let client: p::ClientHello = codec::decode(&packet.payload, s.ceiling()).unwrap();
        assert!(client
            .required_profiles
            .iter()
            .any(|p| p == "mirror2d.core.v1"));
        assert!(client
            .required_profiles
            .iter()
            .any(|p| p == "semantics.only.v1"));
        assert!(
            !client.supported_profiles.iter().any(|p| p.contains("glyph")
                || p.contains("raw")
                || p.contains("composition")
                || p.contains("resize"))
        );
        let packet: p::Packet = codec::decode(&hello(2, true), s.ceiling()).unwrap();
        let mut server: p::ServerHello = codec::decode(&packet.payload, s.ceiling()).unwrap();
        server.selected_profiles = SEMANTIC_PROFILES.iter().map(|p| (*p).into()).collect();
        s.receive(&bound(p::Kind::ServerHello, &server)).unwrap();
        let frames = drain(&mut s);
        let open: p::OpenView = codec::decode(&frames[0].payload, s.ceiling()).unwrap();
        assert_eq!(open.mirror_canvas_policy, "semantic-only");
        publish(&mut s, 1, None, false);
        drain(&mut s);
        assert!(s.acquire(0).is_err());
        assert!(s.presented(1).is_err());
        assert!(s.semantic_ready(2).is_err());
        s.semantic_ready(1).unwrap();
        assert_eq!(drain(&mut s)[0].kind, p::Kind::SemanticReady as i32);
        grant(&mut s);
        assert!(s.has_control());
        assert!(s
            .hit_test(Viewport::new(10, 10), Point { x: 1., y: 1. })
            .is_err());
        let sequence = s.command(1, "Activate", Arguments::None).unwrap();
        let packets = drain(&mut s);
        assert!(packets
            .iter()
            .all(|p| p.kind != p::Kind::PresentedAck as i32));
        assert_eq!(packets[0].kind, p::Kind::SemanticCommand as i32);
        let command = pb::SemanticCommand::decode(packets[0].payload.as_slice()).unwrap();
        assert_eq!(command.command_seq, sequence);
        assert_eq!(command.target_id, 1);
        assert_eq!(command.observed_view_revision, 1);
        s.receive(&bound(
            p::Kind::CommandResult,
            &p::CommandResult {
                command_seq: sequence,
                status: "Applied".into(),
                resulting_view_revision: Some(2),
                ..Default::default()
            },
        ))
        .unwrap();
        publish(&mut s, 2, None, true);
        drain(&mut s);
        assert!(!s.has_control());
        assert!(s.command(1, "Activate", Arguments::None).is_err());
        s.semantic_ready(2).unwrap();
        drain(&mut s);
        assert!(s.has_control());
        s.release().unwrap();
        drain(&mut s);
        assert!(!s.has_control());
        s.disconnect();
        assert!(s.semantic_ready(2).is_err());
        s.start(&[7; 32]).unwrap();
        drain(&mut s);
        s.receive(&bound(p::Kind::ServerHello, &server)).unwrap();
        drain(&mut s);
        s.receive(&bound(
            p::Kind::ResetRequired,
            &p::ResetRequired {
                reason: "test".into(),
            },
        ))
        .unwrap();
        assert!(s.acquire(0).is_err());
        s.close();
        assert_eq!(s.memory_used(), 0);
    }
    #[test]
    fn snapshots_transactions_and_presentation_are_distinct() {
        let mut s = session(true);
        assert!(matches!(
            publish(&mut s, 1, None, false),
            Event::Applied { revision: 1 }
        ));
        assert_eq!(drain(&mut s)[0].kind, p::Kind::AppliedAck as i32);
        assert!(s.acquire(0).is_err());
        s.presented(1).unwrap();
        assert_eq!(drain(&mut s)[0].kind, p::Kind::PresentedAck as i32);
        grant(&mut s);
        assert!(s.has_control());
        assert!(matches!(
            publish(&mut s, 2, None, true),
            Event::Applied { revision: 2 }
        ));
        assert!(!s.has_control());
        assert!(s.presented(1).is_err());
        s.presented(2).unwrap();
        assert!(s.has_control());
        s.close();
        assert_eq!(s.memory_used(), 0);
    }
    #[test]
    fn observer_and_foreign_bindings_cannot_dispatch() {
        let mut s = session(false);
        publish(&mut s, 1, None, false);
        s.presented(1).unwrap();
        assert!(s.acquire(0).is_err());
        assert!(s.command(1, "Activate", Arguments::None).is_err());
        let raw = codec::wrap(
            p::Kind::Pong,
            &p::Ping { nonce: 1 },
            Some(([9; 16], [2; 16], [4; 16])),
        )
        .encode_to_vec();
        assert_eq!(s.receive(&raw).unwrap_err(), TransportError::Binding);
        assert!(!s.has_control());
        assert_eq!(s.state().unwrap().revision, 1);
    }
    #[test]
    fn pending_resources_keep_old_revision_until_exact_verified_bytes() {
        let mut s = session(true);
        publish(&mut s, 1, None, false);
        drain(&mut s);
        let mut bytes = b"RVP1".to_vec();
        bytes.extend(1u32.to_be_bytes());
        bytes.push(1);
        bytes.extend(0f64.to_be_bytes());
        bytes.extend(0f64.to_be_bytes());
        let digest = resource_digest(
            ResourceKind::VectorPath,
            1,
            ResourceProvenance::AuthoredVector,
            &bytes,
        );
        assert!(matches!(
            publish(&mut s, 2, Some(digest), false),
            Event::ResourcesNeeded { revision: 2 }
        ));
        assert_eq!(s.state().unwrap().revision, 1);
        assert_eq!(drain(&mut s)[0].kind, p::Kind::NeedResources as i32);
        s.receive(&bound(
            p::Kind::ResourceOffer,
            &p::ResourceOffer {
                digest: digest.to_vec(),
                kind: 1,
                format_version: 1,
                total_bytes: bytes.len() as u64,
                chunk_count: 1,
                provenance: 1,
            },
        ))
        .unwrap();
        s.receive(&bound(
            p::Kind::ResourceChunk,
            &p::ResourceChunk {
                digest: digest.to_vec(),
                index: 0,
                data: bytes,
            },
        ))
        .unwrap();
        assert!(matches!(
            s.receive(&bound(
                p::Kind::ResourceComplete,
                &p::ResourceComplete {
                    digest: digest.to_vec()
                }
            ))
            .unwrap(),
            Event::Applied { revision: 2 }
        ));
        assert_eq!(s.state().unwrap().revision, 2);
        let kinds: Vec<_> = drain(&mut s).iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            vec![
                p::Kind::ResourceAvailable as i32,
                p::Kind::AppliedAck as i32
            ]
        );
    }
    #[test]
    fn unsolicited_resources_and_malformed_packets_fail_closed() {
        let mut s = session(true);
        assert!(s
            .receive(&bound(
                p::Kind::ResourceOffer,
                &p::ResourceOffer {
                    digest: vec![7; 32],
                    kind: 1,
                    format_version: 1,
                    total_bytes: 25,
                    chunk_count: 1,
                    provenance: 1
                }
            ))
            .is_err());
        assert!(s.receive(&[0xff]).is_err());
        assert!(!s.connected);
        s.close();
        assert_eq!(s.memory_used(), 0);
    }
    #[test]
    fn ambiguous_commands_resume_by_query_and_retry_exact_original_bytes() {
        let mut s = session(true);
        publish(&mut s, 1, None, false);
        s.presented(1).unwrap();
        grant(&mut s);
        let seq = s.command(1, "Activate", Arguments::None).unwrap();
        let original = drain(&mut s).last().unwrap().payload.clone();
        assert!(s.command(1, "Activate", Arguments::None).is_err());
        s.disconnect();
        assert_eq!(s.pending_commands(), 1);
        s.start(&[7; 32]).unwrap();
        drain(&mut s);
        s.receive(&hello(2, true)).unwrap();
        let frames = drain(&mut s);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].kind, p::Kind::OpenView as i32);
        assert_eq!(frames[1].kind, p::Kind::GetCommandResult as i32);
        s.receive(&bound(
            p::Kind::ResumeAccepted,
            &p::ResumeAccepted {
                from_revision: 1,
                through_revision: 1,
                requires_resources: false,
            },
        ))
        .unwrap();
        drain(&mut s);
        s.retry_pending().unwrap();
        assert_eq!(drain(&mut s)[0].payload, original);
        assert!(!s.has_control());
        s.receive(&bound(
            p::Kind::CommandResult,
            &p::CommandResult {
                command_seq: seq,
                status: "Applied".into(),
                source_commit: "source-evidence".into(),
                resulting_view_revision: Some(2),
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(s.pending_commands(), 0);
        s.close();
        assert_eq!(s.memory_used(), 0);
    }
    #[test]
    fn ambiguous_native_statuses_keep_original_command_bytes() {
        for status in ["Unknown", "OutcomeUnknown", "OutcomeExpired", "DISPATCHED"] {
            let mut s = session(true);
            publish(&mut s, 1, None, false);
            s.presented(1).unwrap();
            grant(&mut s);
            let seq = s.command(1, "Activate", Arguments::None).unwrap();
            let original = drain(&mut s).last().unwrap().payload.clone();
            s.receive(&bound(
                p::Kind::CommandResult,
                &p::CommandResult {
                    command_seq: seq,
                    status: status.into(),
                    ..Default::default()
                },
            ))
            .unwrap();
            assert_eq!(s.pending_commands(), 1, "{status}");
            assert!(s.command(1, "Activate", Arguments::None).is_err());
            s.retry_pending().unwrap();
            assert_eq!(drain(&mut s)[0].payload, original);
            s.close();
            assert_eq!(s.memory_used(), 0);
        }
    }
    #[test]
    fn new_epoch_retires_old_commands_and_new_authority_is_required() {
        let mut s = session(true);
        publish(&mut s, 1, None, false);
        s.presented(1).unwrap();
        grant(&mut s);
        s.command(1, "Activate", Arguments::None).unwrap();
        s.disconnect();
        s.start(&[7; 32]).unwrap();
        drain(&mut s);
        s.receive(&hello(9, true)).unwrap();
        assert_eq!(s.pending_commands(), 0);
        assert!(!s.has_control());
        assert_eq!(s.state().unwrap().revision, 0);
        assert_eq!(drain(&mut s)[0].kind, p::Kind::OpenView as i32);
    }
    #[test]
    fn release_during_renewal_does_not_regrant_a_cancelled_request() {
        let mut s = session(true);
        publish(&mut s, 1, None, false);
        s.presented(1).unwrap();
        let mut g = grant(&mut s);
        s.tick(2200).unwrap();
        let frames = drain(&mut s);
        let renew: p::RenewControl = codec::decode(&frames[0].payload, s.ceiling()).unwrap();
        g.request_id = renew.request_id;
        s.release().unwrap();
        drain(&mut s);
        assert!(matches!(
            s.receive(&bound(p::Kind::ControlGranted, &g)).unwrap(),
            Event::None
        ));
        assert!(!s.has_control());
        assert_eq!(drain(&mut s)[0].kind, p::Kind::ReleaseControl as i32);
    }
}
