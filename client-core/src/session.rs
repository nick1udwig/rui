//! Bounded session actor; caller supplies authenticated ordered bytes, clock and presentation.
use crate::replica::{ClientEvent, Replica, Replies};
use crate::{interaction, wire_command::wire_command, Viewport};
use prost::Message;
use rui_protocol_core::{
    commands::{Arguments, CommandFamily, Id},
    model::{Point, Record, State},
    receiver::{ReceiverConfig, ViewReceiver},
    resources::{MemoryBudget, MemoryReservation, ResourceContext},
    session::ReceiverLimits,
};
use rui_wire::{codec, proto as p, WireError as TransportError};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
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
    "browser.semantic.v1",
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
    replica: Option<Replica>,
    outbound: VecDeque<Outbound>,
    outbound_bytes: usize,
    connected: bool,
    starting: bool,
    awaiting_resume: Option<u64>,
    can_control: bool,
    request: Option<Id>,
    cancelled_request: Option<Id>,
    retained_limits: Option<ReceiverLimits>,
    serial: u64,
    request_namespace: Option<Id>,
    renewed_at: u64,
    profiles: Vec<String>,
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
            replica: None,
            outbound: VecDeque::new(),
            outbound_bytes: 0,
            connected: false,
            starting: false,
            awaiting_resume: None,
            can_control: false,
            request: None,
            cancelled_request: None,
            retained_limits: None,
            serial: 1,
            request_namespace: None,
            renewed_at: 0,
            profiles: Vec::new(),
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
        let resume = self.context.zip(self.replica.as_ref()).map(|(c, r)| {
            let revision = r.receiver().state().map(|s| s.revision).unwrap_or(0);
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
            .replica
            .as_ref()
            .map(|r| {
                r.receiver()
                    .memory_budget()
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
        self.replica
            .as_ref()
            .map_or(0, |r| r.receiver().memory_used())
    }
    pub fn receiver(&self) -> Result<&ViewReceiver, TransportError> {
        self.replica
            .as_ref()
            .map(Replica::receiver)
            .ok_or(TransportError::Closed)
    }
    pub fn memory_peak(&self) -> usize {
        self.replica
            .as_ref()
            .map_or(0, |r| r.receiver().memory_budget().peak())
    }
    pub fn state(&self) -> Result<&State, TransportError> {
        self.receiver()?
            .state()
            .map_err(|_| TransportError::Receiver)
    }
    fn ready(&self) -> Option<u64> {
        self.replica.as_ref().and_then(Replica::ready)
    }
    fn lease(&self) -> Option<&p::ControlGranted> {
        self.replica.as_ref().and_then(Replica::lease)
    }
    pub fn has_control(&self) -> bool {
        self.connected
            && self.lease().is_some()
            && self.ready() == self.state().ok().map(|s| s.revision)
    }
    pub fn pending_commands(&self) -> usize {
        self.replica.as_ref().map_or(0, |r| r.commands().len())
    }
    fn replica_io<T>(
        &mut self,
        run: impl FnOnce(&mut Replica, &mut PortableReplies<'_>) -> Result<T, TransportError>,
    ) -> Result<T, TransportError> {
        if !self.connected {
            return Err(TransportError::Closed);
        }
        let replica = self.replica.as_mut().ok_or(TransportError::Closed)?;
        let mut replies = PortableReplies {
            outbound: &mut self.outbound,
            bytes: &mut self.outbound_bytes,
            limits: &self.limits,
            budget: replica.receiver().memory_budget(),
        };
        run(replica, &mut replies)
    }
    pub fn resource_cache_lost(&mut self) -> Result<Event, TransportError> {
        let noop = self
            .receiver()?
            .cache_bytes()
            .map_err(|_| TransportError::Receiver)?
            == 0
            && self
                .receiver()?
                .resources_ready()
                .map_err(|_| TransportError::Receiver)?;
        self.replica_io(|r, q| r.resource_cache_lost(q))?;
        if noop {
            return Ok(Event::None);
        }
        if let Some(r) = self.replica.as_mut() {
            r.clear_readiness();
        }
        let revision = self.state()?.revision;
        if self
            .receiver()?
            .resources_ready()
            .map_err(|_| TransportError::Receiver)?
        {
            Ok(Event::Applied { revision })
        } else {
            Ok(Event::ResourcesNeeded { revision })
        }
    }
    pub fn evict_unpinned_resource(&mut self) -> Result<Option<[u8; 32]>, TransportError> {
        self.replica_io(|r, q| r.evict_unpinned_resource(q))
    }
    fn handshake(&mut self, raw: &[u8]) -> Result<Event, TransportError> {
        let initial: p::Packet = codec::decode(raw, self.limits.envelope_bytes)?;
        if initial.kind == p::Kind::Error as i32 {
            let e: p::Error = codec::decode(&initial.payload, self.limits.envelope_bytes)?;
            return Ok(Event::Error { code: e.code });
        }
        let offered = rui_protocol_core::session::ClientHello {
            client_instance_id: self.client,
            supported_versions: [1].into(),
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
            receiver_limits: self.limits.clone(),
            resume: None,
        };
        let crate::replica::Negotiated {
            mut server,
            limits,
            context,
        } = crate::replica::negotiate_server(raw, &offered, false)?;
        let permissions = server.permissions.take().ok_or(TransportError::Protocol)?;
        let hello = server;
        if self.context == Some(context) {
            if self.profiles != hello.selected_profiles
                || self.retained_limits.as_ref() != Some(&limits)
            {
                return Err(TransportError::Protocol);
            }
            self.replica
                .as_mut()
                .unwrap()
                .configure(&hello.selected_profiles, &permissions)?;

            self.replica
                .as_mut()
                .ok_or(TransportError::Receiver)?
                .reconnect(self.awaiting_resume.take())?;
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
            let mut replica = Replica::new(
                context,
                limits.clone(),
                self.mode == PresentationMode::SemanticOnly,
                receiver,
                self.awaiting_resume.take(),
            )?;
            replica.configure(&hello.selected_profiles, &permissions)?;
            if let Some(old) = self.replica.as_mut() {
                replica.inherit_commands(old)?;
            }
            self.replica = Some(replica);
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
            .replica
            .as_ref()
            .unwrap()
            .commands()
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
        let event = self.replica_io(|r, q| r.receive(raw, q))?;
        self.replica_event(event)
    }
    fn replica_event(&mut self, event: Option<ClientEvent>) -> Result<Event, TransportError> {
        let Some(event) = event else {
            return Ok(Event::None);
        };
        match event {
            ClientEvent::ViewOpened(v) => Ok(Event::ViewOpened {
                width: v.canvas_width,
                height: v.canvas_height,
            }),
            ClientEvent::Applied { revision } => {
                if let Some(r) = self.replica.as_mut() {
                    r.clear_readiness();
                }
                Ok(Event::Applied { revision })
            }
            ClientEvent::ResourcesNeeded { revision } => {
                if let Some(r) = self.replica.as_mut() {
                    r.clear_readiness();
                }
                Ok(Event::ResourcesNeeded { revision })
            }
            ClientEvent::ResourceAvailable => {
                self.replica_io(|r, q| r.queue_resource_request(q))?;
                Ok(Event::None)
            }
            ClientEvent::ResumeAccepted(_) => {
                let follow = self.replica_io(|r, q| r.poll(q))?;
                if matches!(follow, Some(ClientEvent::ResourcesNeeded { .. })) {
                    return self.replica_event(follow);
                }
                if let Some(r) = self.replica.as_mut() {
                    r.clear_readiness();
                }
                Ok(Event::Resumed)
            }
            ClientEvent::ResetRequired(_) => {
                if let Some(r) = self.replica.as_mut() {
                    r.clear_readiness();
                }
                Ok(Event::Reset)
            }
            ClientEvent::ControlGranted(g) => {
                let request = codec::id(&g.request_id)?;
                if self.cancelled_request == Some(request) {
                    self.cancelled_request = None;
                    self.replica.as_mut().unwrap().release_lease();
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

                Ok(Event::Control { granted: true })
            }
            ClientEvent::ControlRevoked(_) => Ok(Event::Control {
                granted: self.lease().is_some(),
            }),
            ClientEvent::CommandResult(r) => Ok(Event::CommandResult {
                sequence: r.command_seq,
                status: r.status,
                rejection: r.rejection,
                source_commit: r.source_commit,
                revision: r.resulting_view_revision,
            }),
            ClientEvent::Error(code) => {
                self.cancelled_request = self.request.take();
                if let Some(r) = self.replica.as_mut() {
                    r.release_lease();
                }
                Ok(Event::Error { code })
            }
            ClientEvent::Pong(_) => Ok(Event::None),
        }
    }
    pub fn presented(&mut self, revision: u64) -> Result<(), TransportError> {
        if self.mode != PresentationMode::Mirror {
            return Err(TransportError::Authority);
        }
        if !self.connected || self.state()?.revision != revision {
            return Err(TransportError::Receiver);
        }
        self.replica_io(|r, q| r.presented(revision, q))?;
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
        self.replica_io(|r, q| r.semantic_ready(revision, q))?;
        Ok(())
    }
    /// Supply a fresh platform-generated namespace once before connecting. This
    /// separates a newly created actor's control requests from retained host
    /// request outcomes for the same approved client identity, even in one view.
    pub fn set_request_namespace(&mut self, namespace: Id) -> Result<(), TransportError> {
        if namespace == [0; 16]
            || self.request_namespace.is_some()
            || self.starting
            || self.context.is_some()
        {
            return Err(TransportError::Protocol);
        }
        self.request_namespace = Some(namespace);
        Ok(())
    }
    fn request_id(&mut self) -> Result<Id, TransportError> {
        let namespace = self.request_namespace.ok_or(TransportError::Authority)?;
        let context = self.context.ok_or(TransportError::Authority)?;
        let mut hash = Sha256::new();
        hash.update(b"RUI/0.1/portable-control-request\0");
        hash.update(namespace);
        hash.update(self.client);
        hash.update(context.scope.session_id);
        hash.update(context.scope.epoch);
        hash.update(context.view_id);
        hash.update(self.serial.to_be_bytes());
        let id = hash.finalize()[..16].try_into().unwrap();
        self.serial = self.serial.checked_add(1).ok_or(TransportError::Limit)?;
        Ok(id)
    }
    pub fn acquire(&mut self, now: u64) -> Result<(), TransportError> {
        if !self.can_control
            || self.ready() != Some(self.state()?.revision)
            || self.request.is_some()
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
        if let Some(g) = self.lease().cloned() {
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
        if let Some(r) = self.replica.as_mut() {
            r.clear_readiness();
        }
        self.cancelled_request = self.request.take();
        if let Some(g) = self.replica.as_mut().and_then(Replica::release_lease) {
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
            self.ready().ok_or(TransportError::Authority)?,
            viewport,
            point,
        )
        .map(|t| t.semantic)
        .map_err(|_| TransportError::Authority)
    }
    pub fn command_binding(&self) -> Result<interaction::CommandBinding, TransportError> {
        let c = self.context.ok_or(TransportError::Binding)?;
        let g = self.lease().ok_or(TransportError::Authority)?;
        Ok(interaction::CommandBinding {
            epoch: c.scope.epoch,
            view: c.view_id,
            client: self.client,
            lease: codec::id(&g.lease_id)?,
            generation: g.generation,
            sequence: self
                .replica
                .as_ref()
                .ok_or(TransportError::Closed)?
                .commands()
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
        if self.ready() != Some(self.state()?.revision) || self.pending_commands() != 0 {
            return Err(TransportError::Authority);
        }
        if !self
            .replica
            .as_ref()
            .and_then(Replica::lease)
            .is_some_and(|g| g.allowed_actions.iter().any(|a| a == action))
        {
            return Err(TransportError::Authority);
        }
        let command =
            interaction::command(self.state()?, target, action, args, self.command_binding()?)
                .map_err(|_| TransportError::Authority)?;
        let wire = wire_command(command);
        let q = self.replica.as_mut().unwrap().commands_mut();
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
        let q = self
            .replica
            .as_ref()
            .ok_or(TransportError::Closed)?
            .commands();
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
        if let Some(r) = self.replica.as_mut() {
            r.release_lease();
        }
        self.request = None;
        self.cancelled_request = None;
        if let Some(r) = self.replica.as_mut() {
            r.clear_readiness();
        }
        for f in &mut self.outbound {
            f.bytes.fill(0);
        }
        self.outbound.clear();
        self.outbound_bytes = 0;
        if let Some(r) = &mut self.replica {
            r.disconnect();
        }
    }
    pub fn close(&mut self) {
        self.disconnect();
        if let Some(r) = &mut self.replica {
            r.close();
        }
        self.replica = None;
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

struct PortableReplies<'a> {
    outbound: &'a mut VecDeque<Outbound>,
    bytes: &'a mut usize,
    limits: &'a ReceiverLimits,
    budget: MemoryBudget,
}
impl Replies for PortableReplies<'_> {
    type Reservation = (MemoryReservation, usize);
    fn reserve(
        &self,
        kind: p::Kind,
        length: usize,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<Self::Reservation, TransportError> {
        let bytes = crate::replica::message_size(kind, length, binding)?;
        if bytes > self.limits.envelope_bytes
            || self.outbound.len() >= 32
            || self
                .bytes
                .checked_add(bytes)
                .is_none_or(|n| n > self.limits.outbound_queue_bytes)
        {
            return Err(TransportError::Limit);
        }
        let charge = bytes
            .checked_mul(4)
            .and_then(|n| n.checked_add(4096))
            .ok_or(TransportError::Limit)?;
        Ok((
            self.budget
                .reserve(charge)
                .map_err(|_| TransportError::Limit)?,
            bytes,
        ))
    }
    fn enqueue_reserved<T: Message>(
        &mut self,
        kind: p::Kind,
        binding: Option<(Id, Id, Id)>,
        (memory, length): Self::Reservation,
        make: impl FnOnce() -> T,
    ) -> Result<(), TransportError> {
        let packet = codec::wrap(kind, &make(), binding);
        self.push(packet, memory, length)
    }
    fn enqueue<T: Message>(
        &mut self,
        kind: p::Kind,
        payload: &T,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<(), TransportError> {
        let (memory, length) = self.reserve(kind, payload.encoded_len(), binding)?;
        self.push(codec::wrap(kind, payload, binding), memory, length)
    }
}
impl PortableReplies<'_> {
    fn push(
        &mut self,
        packet: p::Packet,
        memory: MemoryReservation,
        length: usize,
    ) -> Result<(), TransportError> {
        if packet.encoded_len() != length {
            return Err(TransportError::Protocol);
        }
        self.outbound.push_back(Outbound {
            bytes: packet.encode_to_vec(),
            _memory: Some(memory),
        });
        *self.bytes += length;
        Ok(())
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
        s.set_request_namespace([9; 16]).unwrap();
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
    #[test]
    fn fresh_actor_control_namespace_does_not_reuse_retained_request_ids() {
        let mut old = session(true);
        assert!(old.memory_peak() >= old.memory_used());
        let first = old.request_id().unwrap();
        assert_ne!(first, old.request_id().unwrap());
        old.disconnect();
        old.start(&[7; 32]).unwrap();
        drain(&mut old);
        old.receive(&hello(2, true)).unwrap();
        assert_ne!(first, old.request_id().unwrap());
        let mut fresh = Session::new([3; 16], "fixture".into()).unwrap();
        assert!(fresh.set_request_namespace([0; 16]).is_err());
        fresh.set_request_namespace([10; 16]).unwrap();
        assert!(fresh.set_request_namespace([11; 16]).is_err());
        fresh.start(&[7; 32]).unwrap();
        drain(&mut fresh);
        fresh.receive(&hello(2, true)).unwrap();
        assert_ne!(first, fresh.request_id().unwrap());
        assert!(old.set_request_namespace([12; 16]).is_err());
        let mut missing = Session::new([3; 16], "fixture".into()).unwrap();
        missing.start(&[7; 32]).unwrap();
        drain(&mut missing);
        missing.receive(&hello(2, true)).unwrap();
        assert_eq!(missing.request_id(), Err(TransportError::Authority));
        assert!(missing.set_request_namespace([10; 16]).is_err());
        missing.close();
        assert_eq!(missing.memory_peak(), 0);
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
                browser_input_type: None,
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
        s.set_request_namespace([9; 16]).unwrap();
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
    fn browser_dispatch_receipt_and_high_water_are_negotiated() {
        for browser in [false, true] {
            let mut s = session(true);
            if browser {
                let r = s.replica.as_mut().unwrap();
                let mut profiles = r.profiles().to_vec();
                profiles.push("browser.semantic.v1".into());
                let permissions = r.permissions().unwrap().clone();
                r.configure(&profiles, &permissions).unwrap();
            }
            // Authenticated ViewOpened advances only the browser profile's queue.
            let opened = p::ViewOpened {
                view_id: vec![4; 16],
                canvas_width: 800.,
                canvas_height: 600.,
                coverage: "semantic-only".into(),
                next_command_sequence: Some(42),
                ..Default::default()
            };
            s.receive(&bound(p::Kind::ViewOpened, &opened)).unwrap();
            publish(&mut s, 1, None, false);
            drain(&mut s);
            s.presented(1).unwrap();
            drain(&mut s);
            grant(&mut s);
            let seq = s.command(1, "Activate", Arguments::None).unwrap();
            assert_eq!(seq, if browser { 42 } else { 1 });
            drain(&mut s);
            s.receive(&bound(
                p::Kind::CommandResult,
                &p::CommandResult {
                    command_seq: seq,
                    status: "Dispatched".into(),
                    ..Default::default()
                },
            ))
            .unwrap();
            assert_eq!(s.pending_commands(), if browser { 0 } else { 1 });
        }
    }
    #[test]
    fn native_actor_and_portable_session_agree_at_every_transcript_prefix() {
        let mut s = session(true);
        let limits = s.limits.clone();
        let context = s.context.unwrap();
        let receiver = ViewReceiver::new(
            context,
            limits.clone(),
            ReceiverConfig {
                single_glyph_outlines: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut native = Replica::new(context, limits.clone(), false, receiver, None).unwrap();
        let budget = native.receiver().memory_budget();
        let mut frames = VecDeque::new();
        let mut bytes = 0;
        let mut replies = PortableReplies {
            outbound: &mut frames,
            bytes: &mut bytes,
            limits: &limits,
            budget: budget.clone(),
        };
        for revision in 1..=2 {
            let payload = if revision == 1 {
                pb::Snapshot {
                    revision,
                    records: vec![record(revision, None)],
                    ..Default::default()
                }
                .encode_to_vec()
            } else {
                pb::Transaction {
                    base_revision: revision - 1,
                    new_revision: revision,
                    mutations: vec![pb::Mutation {
                        mutation: Some(pb::mutation::Mutation::Put(record(revision, None))),
                    }],
                    ..Default::default()
                }
                .encode_to_vec()
            };
            let digest = publication_digest(&payload).to_vec();
            let id = vec![revision as u8; 16];
            let tape = [
                bound(
                    if revision == 1 {
                        p::Kind::SnapshotBegin
                    } else {
                        p::Kind::TransactionBegin
                    },
                    &p::PublicationBegin {
                        publication_id: id.clone(),
                        base_revision: revision - 1,
                        revision,
                        total_bytes: payload.len() as u64,
                        chunk_count: 1,
                        item_count: 1,
                        digest: digest.clone(),
                    },
                ),
                bound(
                    p::Kind::PublicationChunk,
                    &p::PublicationChunk {
                        publication_id: id.clone(),
                        index: 0,
                        data: payload.clone(),
                    },
                ),
                bound(
                    p::Kind::PublicationCommit,
                    &p::PublicationCommit {
                        publication_id: id,
                        total_bytes: payload.len() as u64,
                        chunk_count: 1,
                        item_count: 1,
                        digest,
                    },
                ),
            ];
            for (index, raw) in tape.iter().enumerate() {
                let portable = s.receive(raw).unwrap();
                let actual = native.receive(raw, &mut replies).unwrap();
                assert_eq!(s.state().unwrap(), native.receiver().state().unwrap());
                if index == 2 {
                    assert!(matches!(portable, Event::Applied { revision: r } if r == revision));
                    assert!(
                        matches!(actual, Some(ClientEvent::Applied { revision: r }) if r == revision)
                    );
                } else {
                    assert!(matches!(portable, Event::None));
                    assert!(actual.is_none());
                }
                let portable: Vec<_> = drain(&mut s).iter().map(Message::encode_to_vec).collect();
                let actual: Vec<_> = replies.outbound.drain(..).map(|f| f.bytes).collect();
                *replies.bytes = 0;
                assert_eq!(portable, actual);
            }
            s.presented(revision).unwrap();
            native.presented(revision, &mut replies).unwrap();
            assert_eq!(
                drain(&mut s)[0].encode_to_vec(),
                replies.outbound.pop_front().unwrap().bytes
            );
            *replies.bytes = 0;
            assert_eq!(s.ready(), native.ready());
        }
        let granted = grant(&mut s);
        native
            .receive(&bound(p::Kind::ControlGranted, &granted), &mut replies)
            .unwrap();
        assert_eq!(s.lease().unwrap(), native.lease().unwrap());
        let sequence = s.command(1, "Activate", Arguments::None).unwrap();
        let packet = drain(&mut s).remove(0);
        native
            .retain_command(CommandFamily::Semantic, &packet.payload)
            .unwrap();
        for status in ["Unknown", "OutcomeUnknown", "OutcomeExpired", "Dispatched"] {
            let raw = bound(
                p::Kind::CommandResult,
                &p::CommandResult {
                    command_seq: sequence,
                    status: status.into(),
                    ..Default::default()
                },
            );
            s.receive(&raw).unwrap();
            native.receive(&raw, &mut replies).unwrap();
            assert_eq!(s.pending_commands(), native.commands().len());
            assert_eq!(native.commands().latest().unwrap().bytes(), packet.payload);
        }
        // Peer failure clears both readiness/leases while preserving the replica
        // and exact ambiguous command. Further packets cannot revive this socket.
        let foreign = codec::wrap(
            p::Kind::Pong,
            &p::Ping { nonce: 1 },
            Some(([9; 16], [2; 16], [4; 16])),
        )
        .encode_to_vec();
        assert_eq!(s.receive(&foreign).unwrap_err(), TransportError::Binding);
        assert_eq!(
            native.receive(&foreign, &mut replies).unwrap_err(),
            TransportError::Binding
        );
        let good = bound(p::Kind::Pong, &p::Ping { nonce: 2 });
        assert_eq!(s.receive(&good).unwrap_err(), TransportError::Closed);
        assert_eq!(
            native.receive(&good, &mut replies).unwrap_err(),
            TransportError::Closed
        );
        assert!(native.lease().is_none());
        assert!(native.ready().is_none());
        assert!(!s.has_control());
        assert_eq!(s.state().unwrap(), native.receiver().state().unwrap());
        assert_eq!(native.commands().len(), 1);
        assert_eq!(s.pending_commands(), 1);
        s.close();
        native.close();
        assert_eq!(budget.used(), 0);
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
    fn cache_loss_preserves_commands_and_requires_verified_closure_then_presentation() {
        let mut s = session(true);
        publish(&mut s, 1, None, false);
        drain(&mut s);
        s.presented(1).unwrap();
        drain(&mut s);
        grant(&mut s);
        s.command(1, "Activate", Arguments::None).unwrap();
        let original = drain(&mut s).pop().unwrap().payload.clone();
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
        publish(&mut s, 2, Some(digest), false);
        drain(&mut s);
        let refill = |s: &mut Session| {
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
                    data: bytes.clone(),
                },
            ))
            .unwrap();
            s.receive(&bound(
                p::Kind::ResourceComplete,
                &p::ResourceComplete {
                    digest: digest.to_vec(),
                },
            ))
            .unwrap()
        };
        assert!(matches!(refill(&mut s), Event::Applied { revision: 2 }));
        drain(&mut s);
        s.presented(2).unwrap();
        drain(&mut s);
        assert!(s.has_control());
        assert_eq!(s.pending_commands(), 1);
        assert!(matches!(
            s.resource_cache_lost().unwrap(),
            Event::ResourcesNeeded { revision: 2 }
        ));
        assert!(!s.has_control());
        assert_eq!(s.state().unwrap().revision, 2);
        assert_eq!(s.pending_commands(), 1);
        assert!(s.presented(2).is_err());
        let frames = drain(&mut s);
        assert_eq!(
            frames.iter().map(|p| p.kind).collect::<Vec<_>>(),
            vec![
                p::Kind::ResourceEvicted as i32,
                p::Kind::NeedResources as i32
            ]
        );
        for p in frames {
            assert_eq!(
                codec::decode::<p::ResourceIds>(&p.payload, s.ceiling())
                    .unwrap()
                    .digests,
                vec![digest.to_vec()]
            );
        }
        assert!(matches!(refill(&mut s), Event::Applied { revision: 2 }));
        drain(&mut s);
        assert!(!s.has_control());
        s.presented(2).unwrap();
        drain(&mut s);
        assert!(s.has_control());
        s.retry_pending().unwrap();
        assert_eq!(drain(&mut s).pop().unwrap().payload, original);
        s.resource_cache_lost().unwrap();
        drain(&mut s);
        s.disconnect();
        s.start(&[7; 32]).unwrap();
        drain(&mut s);
        s.receive(&hello(2, true)).unwrap();
        drain(&mut s);
        assert!(matches!(
            s.receive(&bound(
                p::Kind::ResumeAccepted,
                &p::ResumeAccepted {
                    from_revision: 2,
                    through_revision: 2,
                    requires_resources: true,
                }
            ))
            .unwrap(),
            Event::ResourcesNeeded { revision: 2 }
        ));
        assert!(!s.has_control());
        drain(&mut s);
        assert!(matches!(refill(&mut s), Event::Applied { revision: 2 }));
        drain(&mut s);
        assert!(!s.has_control(), "recovery silently restored the old lease");
        assert!(s.evict_unpinned_resource().unwrap().is_none());
        s.close();
        assert_eq!(s.memory_used(), 0);
    }
    #[test]
    fn paged_512_body_closure_and_cache_loss_never_eagerly_fill_the_queue() {
        let mut s = Session::new([3; 16], "fixture".into()).unwrap();
        s.start(&[7; 32]).unwrap();
        drain(&mut s);
        let packet: p::Packet = codec::decode(&hello(2, false), 128 << 10).unwrap();
        let mut server: p::ServerHello = codec::decode(&packet.payload, 128 << 10).unwrap();
        server.negotiated_limits.as_mut().unwrap().envelope_bytes = 1024;
        s.receive(&bound(p::Kind::ServerHello, &server)).unwrap();
        drain(&mut s);
        let mut bodies = Vec::new();
        let mut records = Vec::new();
        for i in 0..512 {
            let mut bytes = b"RVP1".to_vec();
            bytes.extend(1u32.to_be_bytes());
            bytes.push(1);
            bytes.extend((i as f64).to_be_bytes());
            bytes.extend(0f64.to_be_bytes());
            let digest = resource_digest(
                ResourceKind::VectorPath,
                1,
                ResourceProvenance::AuthoredVector,
                &bytes,
            );
            let mut r = record(1, Some(digest));
            if let Some(pb::record::Record::Scene(n)) = &mut r.record {
                n.id = i + 2;
            }
            records.push(r);
            bodies.push((digest, bytes));
        }
        let payload = pb::Snapshot {
            revision: 1,
            records,
            ..Default::default()
        }
        .encode_to_vec();
        let hash = publication_digest(&payload);
        let publication = vec![6; 16];
        s.receive(&bound(
            p::Kind::SnapshotBegin,
            &p::PublicationBegin {
                publication_id: publication.clone(),
                base_revision: 0,
                revision: 1,
                total_bytes: payload.len() as u64,
                chunk_count: payload.chunks(256).count() as u32,
                digest: hash.to_vec(),
                item_count: 512,
            },
        ))
        .unwrap();
        for (i, data) in payload.chunks(256).enumerate() {
            s.receive(&bound(
                p::Kind::PublicationChunk,
                &p::PublicationChunk {
                    publication_id: publication.clone(),
                    index: i as u32,
                    data: data.to_vec(),
                },
            ))
            .unwrap();
        }
        assert!(matches!(
            s.receive(&bound(
                p::Kind::PublicationCommit,
                &p::PublicationCommit {
                    publication_id: publication,
                    total_bytes: payload.len() as u64,
                    chunk_count: payload.chunks(256).count() as u32,
                    digest: hash.to_vec(),
                    item_count: 512,
                }
            ))
            .unwrap(),
            Event::ResourcesNeeded { revision: 1 }
        ));
        for lost in [false, true] {
            if lost {
                assert!(matches!(
                    s.resource_cache_lost().unwrap(),
                    Event::ResourcesNeeded { revision: 1 }
                ));
            }
            let mut requested = std::collections::BTreeSet::new();
            let mut evicted = std::collections::BTreeSet::new();
            let mut applied = 0;
            for index in 0..512 {
                let frames = drain(&mut s);
                if index == 0 {
                    assert_eq!(frames.len(), if lost { 2 } else { 1 });
                }
                for p in frames {
                    assert!(p.encoded_len() <= 1024);
                    if p.kind == p::Kind::NeedResources as i32
                        || p.kind == p::Kind::ResourceEvicted as i32
                    {
                        let ids: p::ResourceIds = codec::decode(&p.payload, 1024).unwrap();
                        let set = if p.kind == p::Kind::NeedResources as i32 {
                            &mut requested
                        } else {
                            &mut evicted
                        };
                        for d in ids.digests {
                            assert!(set.insert(d), "duplicate resource page");
                        }
                    }
                }
                let (digest, bytes) = bodies
                    .iter()
                    .find(|(d, _)| {
                        requested.contains(d.as_slice())
                            && !s.receiver().unwrap().read_resource(*d).is_ok()
                    })
                    .unwrap();
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
                        data: bytes.clone(),
                    },
                ))
                .unwrap();
                if matches!(
                    s.receive(&bound(
                        p::Kind::ResourceComplete,
                        &p::ResourceComplete {
                            digest: digest.to_vec()
                        }
                    ))
                    .unwrap(),
                    Event::Applied { revision: 1 }
                ) {
                    applied += 1;
                }
            }
            assert_eq!(requested.len(), 512);
            assert_eq!(evicted.len(), if lost { 512 } else { 0 });
            assert_eq!(applied, 1);
            assert!(s.receiver().unwrap().resources_ready().unwrap());
            drain(&mut s);
            s.presented(1).unwrap();
            drain(&mut s);
        }
        s.close();
        assert_eq!(s.memory_used(), 0);
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
