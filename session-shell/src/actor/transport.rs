//! The same native owner serves authenticated TLS views. It never creates a
//! controlled fixture application, a second broker, or a second command ledger.
mod projection;
use super::*;
pub use projection::ProjectedCapture;
use rui_protocol_core::{
    commands::{Command, CommandFamily, DispatchFailure, Rejection},
    session::InputCleanup,
};
use rui_transport::{
    producer::{self as port, Acknowledgement, AuthorizationLifetime, Control, ProducerPort},
    TransportError,
};
use tokio::sync::Semaphore;

/// A bounded asynchronous facade over the existing !Send native owner thread.
/// Futures poll charged single-response channels; no task is spawned per call.
#[derive(Clone)]
pub struct NativeProducerPort(Arc<Port>);
struct Port {
    host: HostPort,
    admission: Arc<Semaphore>,
    lifecycle: Arc<Semaphore>,
    _memory: MemoryReservation,
}
impl NativeProducerPort {
    pub fn new(host: HostPort) -> Result<Self, ShellError> {
        let memory = host.budget.reserve(64 << 10)?;
        Ok(Self(Arc::new(Port {
            host,
            admission: Arc::new(Semaphore::new(QUEUE)),
            lifecycle: Arc::new(Semaphore::new(1)),
            _memory: memory,
        })))
    }
    /// Independent direct projected records/resources at the last published
    /// revision, not a protobuf encode/decode or receiver replay.
    pub async fn projected_capture(
        &self,
        connection: &port::Connection,
    ) -> Result<ProjectedCapture, TransportError> {
        let native = self.native_connection(connection)?.clone();
        let _admitted = self
            .0
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| TransportError::Limit)?;
        let pending = self
            .0
            .host
            .request(128 << 10, || Operation::Projected { connection: native })
            .map_err(error)?;
        let received = receive(pending).await?;
        match received.value {
            Response::Projected(capture) => Ok(capture),
            _ => Err(TransportError::Protocol),
        }
    }
    /// Trusted source-owner diagnostic capability. The server's current view
    /// must still be connected and approved. This is not a wire API and cannot
    /// admit commands or synthesize receipt/presentation evidence.
    pub async fn projected_source(
        &self,
        client: Id,
        view: Id,
    ) -> Result<ProjectedCapture, TransportError> {
        let _admitted = self
            .0
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| TransportError::Limit)?;
        let pending = self
            .0
            .host
            .request(128 << 10, || Operation::ProjectedSource { client, view })
            .map_err(error)?;
        let received = receive(pending).await?;
        match received.value {
            Response::Projected(capture) => Ok(capture),
            _ => Err(TransportError::Protocol),
        }
    }
    fn native_connection<'a>(
        &self,
        connection: &'a port::Connection,
    ) -> Result<&'a Connection, TransportError> {
        let native = connection
            .capability::<Connection>()
            .ok_or(TransportError::Authority)?;
        self.0.host.priority.validate(native).map_err(error)?;
        if !connection.authorized() {
            return Err(TransportError::Authentication);
        }
        Ok(native)
    }
}
async fn receive(pending: Pending) -> Result<Received, TransportError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match pending.try_recv().map_err(error)? {
            Some(received) => return received.map_err(error),
            None if Instant::now() >= deadline => return Err(TransportError::Deadline),
            None => tokio::time::sleep(Duration::from_millis(1)).await,
        }
    }
}
impl ProducerPort for NativeProducerPort {
    fn transact(&self, request: port::Request) -> port::PortFuture<'_> {
        let permit = match self.0.admission.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Box::pin(std::future::ready(Err(TransportError::Limit))),
        };
        // Own the exact incoming body under the native shared budget before
        // constructing either the queued request or its asynchronous future.
        let bytes = request_memory(&request);
        let pending = match self
            .0
            .host
            .request(bytes, || Operation::Transport(Box::new(request)))
        {
            Ok(pending) => pending,
            Err(err) => return Box::pin(std::future::ready(Err(error(err)))),
        };
        Box::pin(async move {
            let _permit = permit;
            let received = receive(pending).await?;
            match received.value {
                Response::Transport(result) => result,
                _ => Err(TransportError::Protocol),
            }
        })
    }
    fn disconnect(&self, connection: &port::Connection) {
        if let Some(native) = connection.capability::<Connection>() {
            let _ = self.0.host.disconnect(native);
        }
    }
    fn shutdown(&self) {
        self.0.host.priority.quit.store(true, Ordering::Release);
        let _ = self.0.host.priority.invalidate();
    }
    fn flush_lifecycle(&self, _now: u64) -> port::LifecycleFuture<'_> {
        // The native thread continuously owns/drains priority notices. A busy
        // flush does not abandon that obligation or use normal FIFO capacity.
        let permit = match self.0.lifecycle.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Box::pin(std::future::ready(Err(TransportError::Limit))),
        };
        Box::pin(async move {
            let _permit = permit;
            let priority = &self.0.host.priority;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if priority.owner_failed.load(Ordering::Acquire) {
                    return Err(TransportError::Authority);
                }
                if priority.quit.load(Ordering::Acquire) {
                    if priority.stopped.load(Ordering::Acquire) {
                        return Ok(());
                    }
                } else if priority.settled.load(Ordering::Acquire)
                    && priority.pending_requests.load(Ordering::Acquire) == 0
                    && priority.dispatch_ready(priority.generation.load(Ordering::Acquire))
                {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(TransportError::Deadline);
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
    }
}
fn request_memory(request: &port::Request) -> usize {
    let mut bytes = 128 << 10;
    match request {
        port::Request::Connect {
            authorization,
            hello,
            ..
        } => {
            bytes += authorization.principal.capacity();
            for value in authorization
                .permissions
                .actions
                .iter()
                .chain(&hello.supported_profiles)
                .chain(&hello.required_profiles)
            {
                bytes += 512 + value.capacity();
            }
        }
        port::Request::Scoped { operation, .. } => match operation {
            port::Operation::Command { bytes: body, .. } => bytes += body.capacity(),
            port::Operation::Open {
                selection,
                mirror_canvas_policy,
            } => bytes += selection.capacity() + mirror_canvas_policy.capacity(),
            port::Operation::Resources { digests, .. } => bytes += digests.capacity() * 32,
            port::Operation::Evicted {
                digests,
                previously_missing,
                ..
            } => bytes += (digests.capacity() + previously_missing.capacity()) * 32,
            _ => {}
        },
    }
    bytes
}
fn error(error: ShellError) -> TransportError {
    match error {
        ShellError::Limit | ShellError::Memory(_) => TransportError::Limit,
        ShellError::Native("owner stopped") => TransportError::Closed,
        _ => TransportError::Authority,
    }
}

#[derive(Default)]
pub(super) struct NativeViews {
    views: [Option<View>; 8],
}
struct View {
    stamp: ScopeStamp,
    client: AuthenticatedClient,
    lifetime: AuthorizationLifetime,
    limits: rui_protocol_core::model::Limits,
    negotiated_limits: rui_protocol_core::session::ReceiverLimits,
    opened: bool,
    capture: Option<ProjectedCapture>,
    checkpoint: Option<SharedPayload>,
    revoked: Option<port::Revoked>,
    _memory: rui_protocol_core::session::ViewMemoryReservation,
}
pub(super) struct Context<'a> {
    pub backend: &'a mut Backend,
    pub broker: &'a mut SessionBroker,
    pub ledger: &'a mut CommandLedger,
    pub priority: &'a Arc<Priority>,
    pub budget: &'a MemoryBudget,
    pub slots: &'a mut BTreeMap<Id, usize>,
    pub identities: &'a mut [Option<AuthenticatedClient>; 8],
    pub serial: &'a mut u64,
    pub poisoned: bool,
    pub local_held: bool,
    pub generation: u64,
    pub now: u64,
}
impl NativeViews {
    pub(super) fn capture_named(
        &self,
        client: Id,
        view_id: Id,
        budget: &MemoryBudget,
        broker: &SessionBroker,
        priority: &Priority,
    ) -> Result<ProjectedCapture, ShellError> {
        let (index, view) = self
            .views
            .iter()
            .enumerate()
            .find_map(|(index, view)| {
                view.as_ref()
                    .filter(|v| v.stamp.client == client && v.stamp.view == view_id)
                    .map(|v| (index, v))
            })
            .ok_or(ShellError::Scope)?;
        if priority.current.lock().map_err(|_| ShellError::Scope)?[index] != Some(view.stamp)
            || !view.lifetime.is_authorized()
        {
            return Err(ShellError::Scope);
        }
        broker.authorize_projection(&view.client, view_id)?;
        let memory = broker.reserve_view_memory(&view.client, view_id, 2 << 20)?;
        let mut capture =
            projection::copy(view.capture.as_ref().ok_or(ShellError::Scope)?, budget)?;
        capture.charge_view(memory);
        Ok(capture)
    }
    pub(super) fn synchronize(
        &mut self,
        broker: &mut SessionBroker,
        priority: &Priority,
        now: u64,
    ) {
        for view in self.views.iter().flatten() {
            if !view.lifetime.is_authorized() {
                let _ = broker.invalidate_authorization(&view.client, now);
                let current = priority.current.lock().unwrap();
                if let Some(index) = current.iter().position(|s| *s == Some(view.stamp)) {
                    drop(current);
                    let _ = priority.disconnect_stamp(index, view.stamp);
                }
            }
        }
    }
    pub(super) fn revoked(&mut self, cleanup: &InputCleanup) {
        for view in self.views.iter_mut().flatten() {
            if view.stamp.client == cleanup.lease.client.client_instance_id
                && view.stamp.view == cleanup.lease.view_id
            {
                view.revoked = Some(port::Revoked {
                    lease: cleanup.lease.lease_id,
                    generation: cleanup.lease.generation,
                    reason: format!("{:?}", cleanup.reason),
                });
            }
        }
    }
    pub(super) fn capture(
        &self,
        connection: &Connection,
        budget: &MemoryBudget,
        broker: &SessionBroker,
    ) -> Result<ProjectedCapture, ShellError> {
        let view = self.views[connection.0.priority_slot]
            .as_ref()
            .filter(|v| v.stamp == connection.stamp())
            .ok_or(ShellError::Scope)?;
        if !view.lifetime.is_authorized() {
            return Err(ShellError::Scope);
        }
        let memory = broker.reserve_view_memory(connection.client(), connection.view(), 2 << 20)?;
        let mut capture =
            projection::copy(view.capture.as_ref().ok_or(ShellError::Scope)?, budget)?;
        capture.charge_view(memory);
        Ok(capture)
    }
    pub(super) fn execute(
        &mut self,
        request: port::Request,
        mut context: Context<'_>,
    ) -> Result<port::Received, TransportError> {
        let memory = context
            .budget
            .reserve(128 << 10)
            .map_err(|_| TransportError::Limit)?;
        let result = self.execute_inner(request, &mut context)?;
        Ok(port::Received::charged(result, memory, None))
    }
    fn execute_inner(
        &mut self,
        request: port::Request,
        ctx: &mut Context<'_>,
    ) -> Result<port::Reply, TransportError> {
        match request {
            port::Request::Connect {
                mut authorization,
                hello,
                lifetime,
                fresh_view,
                ..
            } => {
                if !lifetime.is_authorized() {
                    return Err(TransportError::Authentication);
                }
                let current = ctx
                    .priority
                    .current
                    .lock()
                    .map_err(|_| TransportError::Authority)?;
                if current
                    .iter()
                    .flatten()
                    .any(|s| s.client == hello.client_instance_id)
                {
                    return Err(TransportError::ProducerBusy);
                }
                let next_slot = ctx.slots.len();
                let index = *ctx
                    .slots
                    .get(&hello.client_instance_id)
                    .unwrap_or(&next_slot);
                if index >= 4 {
                    return Err(TransportError::Limit);
                }
                // Authentication is verified by transport, but client-instance
                // IDs are peer supplied. Never retire another principal's
                // offline view while attempting an explicitly fresh binding.
                if ctx.identities[index]
                    .as_ref()
                    .is_some_and(|identity| identity.principal != authorization.principal)
                {
                    return Err(TransportError::Authentication);
                }
                let global = ctx
                    .budget
                    .reserve(64 << 10)
                    .map_err(|_| TransportError::Limit)?;
                let serial = ctx.serial.checked_add(1).ok_or(TransportError::Limit)?;
                authorization
                    .permissions
                    .actions
                    .retain(|a| a == "RawInputBatch");
                if fresh_view {
                    if let Some(identity) = ctx.identities[index].as_ref() {
                        let _ = ctx.broker.invalidate_authorization(identity, ctx.now);
                    }
                }
                let mut retry_hello = *hello;
                let mut negotiated = ctx
                    .broker
                    .negotiate(Some(authorization.clone()), retry_hello.clone(), ctx.now)
                    .map_err(|_| TransportError::Authority)?;
                if self.views[index].as_ref().is_some_and(|view| {
                    view.stamp.view == negotiated.view_id
                        && view.negotiated_limits != negotiated.limits
                }) {
                    // A retained reducer/profile cannot silently change its
                    // ceilings. Match ControlledHost's REC-005 fresh-view rule.
                    ctx.broker
                        .invalidate_authorization(&negotiated.client, ctx.now)
                        .map_err(|_| TransportError::Authority)?;
                    retry_hello.resume = None;
                    negotiated = ctx
                        .broker
                        .negotiate(Some(authorization), retry_hello, ctx.now)
                        .map_err(|_| TransportError::Authority)?;
                    negotiated.resume = ResumeDisposition::ResetRequired(
                        rui_protocol_core::session::ResetReason::ViewChanged,
                    );
                }
                let held = match ctx.broker.reserve_view_memory(
                    &negotiated.client,
                    negotiated.view_id,
                    64 << 10,
                ) {
                    Ok(held) => held,
                    Err(_) => {
                        let _ = ctx.broker.disconnect(&negotiated.client, ctx.now);
                        return Err(TransportError::Limit);
                    }
                };
                let keeper = if matches!(&negotiated.resume, ResumeDisposition::Accepted { from_revision, through_revision, .. } if from_revision == through_revision)
                {
                    let existing = self.views[index]
                        .as_ref()
                        .filter(|v| v.stamp.view == negotiated.view_id)
                        .and_then(|v| v.capture.as_ref());
                    match existing
                        .map(|capture| keeper(capture, &negotiated, ctx))
                        .transpose()
                    {
                        Ok(Some(keeper)) => Some(keeper),
                        Ok(None) => {
                            let _ = ctx.broker.disconnect(&negotiated.client, ctx.now);
                            return Err(TransportError::Authority);
                        }
                        Err(error) => {
                            let _ = ctx.broker.disconnect(&negotiated.client, ctx.now);
                            return Err(error);
                        }
                    }
                } else {
                    None
                };
                drop(current);
                let native = Connection(Arc::new(ConnectionData {
                    client: negotiated.client.clone(),
                    view: negotiated.view_id,
                    epoch: negotiated.epoch,
                    session: negotiated.session_id,
                    priority_slot: index,
                    owner: ctx.priority.identity.clone(),
                    serial,
                    lifecycle: Arc::downgrade(ctx.priority),
                    _memory: global,
                }));
                let view_memory = ctx
                    .broker
                    .reserve_view_memory(native.client(), native.view(), 64 << 10)
                    .map_err(|_| {
                        let _ = ctx.broker.disconnect(native.client(), ctx.now);
                        TransportError::Limit
                    })?;
                *ctx.serial = serial;
                ctx.slots
                    .insert(negotiated.client.client_instance_id, index);
                ctx.identities[index] = Some(negotiated.client.clone());
                ctx.priority.current.lock().unwrap()[index] = Some(native.stamp());
                if let Some(view) = self.views[index]
                    .as_mut()
                    .filter(|v| v.stamp.view == native.view())
                {
                    view.stamp = native.stamp();
                    view.lifetime = lifetime.clone();
                    view.opened = false;
                } else {
                    self.views[index] = Some(View {
                        stamp: native.stamp(),
                        client: native.client().clone(),
                        lifetime: lifetime.clone(),
                        limits: model_limits(&negotiated.limits),
                        negotiated_limits: negotiated.limits.clone(),
                        opened: false,
                        capture: None,
                        checkpoint: None,
                        revoked: None,
                        _memory: view_memory,
                    });
                }
                Ok(port::Reply::Connected(port::Connection::issued(
                    negotiated,
                    keeper,
                    lifetime,
                    Box::new(native),
                    held,
                )))
            }
            port::Request::Scoped {
                connection,
                operation,
                ..
            } => {
                let native = connection
                    .capability::<Connection>()
                    .ok_or(TransportError::Authority)?;
                ctx.priority.validate(native).map_err(error)?;
                if native.client() != &connection.session().client
                    || native.view() != connection.session().view_id
                    || native.session() != connection.session().session_id
                    || native.epoch() != connection.session().epoch
                {
                    return Err(TransportError::Binding);
                }
                let view = self.views[native.0.priority_slot]
                    .as_mut()
                    .filter(|v| v.stamp == native.stamp())
                    .ok_or(TransportError::Authority)?;
                if !view.lifetime.is_authorized() || !connection.authorized() {
                    return Err(TransportError::Authentication);
                }
                ctx.broker
                    .authorize_projection(native.client(), native.view())
                    .map_err(|_| TransportError::Authority)?;
                match operation {
                    port::Operation::Authorize => Ok(port::Reply::Done),
                    port::Operation::Open {
                        selection,
                        mirror_canvas_policy,
                    } => {
                        if view.opened
                            || selection != "session"
                            || mirror_canvas_policy != "host-canvas"
                        {
                            return Err(TransportError::Authority);
                        }
                        let checkpoint = if matches!(
                            connection.session().resume,
                            ResumeDisposition::Accepted { .. }
                        ) {
                            None
                        } else {
                            Some(snapshot(view, native, ctx)?)
                        };
                        view.opened = true;
                        let logical = ctx.backend.topology().grant().logical();
                        let raw = connection.session().permissions.can_control
                            && connection
                                .session()
                                .permissions
                                .actions
                                .contains("RawInputBatch");
                        Ok(port::Reply::Opened {
                            description: port::ViewDescription {
                                width: logical.width,
                                height: logical.height,
                                coverage: "complete".into(),
                                input_modes: if raw {
                                    vec!["raw.host.v1".into()]
                                } else {
                                    vec![]
                                },
                                input_devices: if raw {
                                    vec![port::InputDeviceDescription {
                                        seat_id: crate::backend::SEAT.into(),
                                        device_id: crate::backend::DEVICE.into(),
                                        key_namespace: "linux.evdev".into(),
                                        input_mode: "raw.host.v1".into(),
                                        text_owner: "HOST_PHYSICAL".into(),
                                    }]
                                } else {
                                    vec![]
                                },
                            },
                            checkpoint,
                        })
                    }
                    port::Operation::Ack(ack) => {
                        match ack {
                            Acknowledgement::Applied { revision, witness } => {
                                if witness.publication_revision().is_some() {
                                    ctx.broker.acknowledge_applied_publication(
                                        native.client(),
                                        native.view(),
                                        revision,
                                        true,
                                        &witness,
                                    )
                                } else {
                                    ctx.broker.acknowledge_applied(
                                        native.client(),
                                        native.view(),
                                        revision,
                                        true,
                                    )
                                }
                            }
                            Acknowledgement::Presented { revision } => ctx
                                .broker
                                .acknowledge_presented(native.client(), native.view(), revision),
                            Acknowledgement::SemanticReady { .. } => {
                                return Err(TransportError::Authority)
                            }
                        }
                        .map_err(|_| TransportError::Authority)?;
                        Ok(port::Reply::Done)
                    }
                    port::Operation::Control(control) => {
                        if ctx.poisoned
                            || ctx.backend.cleanup_pending()
                            || ctx.local_held
                            || !ctx.priority.dispatch_ready(ctx.generation)
                        {
                            return Err(TransportError::Authority);
                        }
                        match control {
                            Control::Acquire { request } => ctx
                                .broker
                                .acquire_control(native.client(), native.view(), request, ctx.now)
                                .map(port::Reply::Lease),
                            Control::Renew {
                                request,
                                lease,
                                generation,
                            } => ctx
                                .broker
                                .renew_control(
                                    native.client(),
                                    native.view(),
                                    request,
                                    lease,
                                    generation,
                                    ctx.now,
                                )
                                .map(port::Reply::Lease),
                            Control::Release {
                                request,
                                lease,
                                generation,
                            } => ctx
                                .broker
                                .release_control(
                                    native.client(),
                                    request,
                                    lease,
                                    generation,
                                    ctx.now,
                                )
                                .map(|_| port::Reply::Done),
                        }
                        .map_err(|_| TransportError::Authority)
                    }
                    port::Operation::Command { family, bytes } => {
                        if ctx.poisoned
                            || ctx.backend.cleanup_pending()
                            || !ctx.priority.dispatch_ready(ctx.generation)
                        {
                            return Err(TransportError::Authority);
                        }
                        let result = if family == CommandFamily::RawInput {
                            ctx.backend
                                .submit_raw_at_generation(
                                    ctx.broker,
                                    ctx.ledger,
                                    native.client(),
                                    &bytes,
                                    ctx.now,
                                    ctx.generation,
                                )
                                .map_err(error)?
                        } else {
                            ctx.ledger
                                .submit_controlled_with_session(
                                    ctx.broker,
                                    native.client(),
                                    family,
                                    &bytes,
                                    ctx.now,
                                    |_: &Command| Err(Rejection::UnsupportedAction),
                                    |_, _| Err(DispatchFailure::Failed),
                                )
                                .map_err(|_| TransportError::Protocol)?
                        };
                        Ok(port::Reply::Command(result))
                    }
                    port::Operation::Result { sequence } => ctx
                        .ledger
                        .get_result(native.client(), native.epoch(), sequence, ctx.now)
                        .map(port::Reply::Command)
                        .map_err(|_| TransportError::Authority),
                    port::Operation::Resources { .. } | port::Operation::Evicted { .. } => {
                        Err(TransportError::Authority)
                    }
                    port::Operation::ResourcesMissing => {
                        ctx.broker
                            .mark_resources_missing(native.client(), native.view())
                            .map_err(|_| TransportError::Authority)?;
                        Ok(port::Reply::Done)
                    }
                    port::Operation::ReserveMemory { bytes } => ctx
                        .broker
                        .reserve_view_memory(native.client(), native.view(), bytes)
                        .map(port::Reply::Memory)
                        .map_err(|_| TransportError::Limit),
                    port::Operation::Pump { take_publication } => {
                        let publication = if take_publication && view.opened {
                            update(view, native, ctx)?
                        } else {
                            None
                        };
                        Ok(port::Reply::Pump {
                            publication,
                            revoked: view.revoked.take(),
                        })
                    }
                }
            }
        }
    }
}
fn envelope(native: &Connection, kind: i32, payload: Vec<u8>) -> Vec<u8> {
    pb::Envelope {
        protocol_version: 1,
        session_id: native.session().to_vec(),
        epoch: native.epoch().to_vec(),
        view_id: native.view().to_vec(),
        kind,
        payload,
        required_features: vec!["cursor.vector.v1".into()],
    }
    .encode_to_vec()
}
fn model_limits(
    negotiated: &rui_protocol_core::session::ReceiverLimits,
) -> rui_protocol_core::model::Limits {
    rui_protocol_core::model::Limits {
        max_envelope_bytes: negotiated.envelope_bytes,
        max_snapshot_bytes: negotiated.snapshot_bytes,
        max_transaction_bytes: negotiated.transaction_bytes,
        max_memory_bytes: negotiated.view_memory_bytes,
        max_records: negotiated.records,
        max_depth: negotiated.ownership_depth,
        ..Default::default()
    }
}
fn validate_wire(
    bytes: &[u8],
    limits: &rui_protocol_core::model::Limits,
) -> Result<(), TransportError> {
    use rui_protocol_core::wire;
    let envelope = wire::decode_envelope(bytes, limits).map_err(|_| TransportError::Limit)?;
    match envelope.kind {
        1 => {
            wire::decode_snapshot(&envelope.payload, limits).map_err(|_| TransportError::Limit)?;
        }
        2 => {
            wire::decode_transaction(&envelope.payload, limits)
                .map_err(|_| TransportError::Limit)?;
        }
        _ => return Err(TransportError::Protocol),
    }
    Ok(())
}
fn keeper(
    capture: &ProjectedCapture,
    negotiated: &NegotiatedSession,
    ctx: &mut Context<'_>,
) -> Result<SharedPayload, TransportError> {
    let _staging = ctx
        .budget
        .reserve(1 << 20)
        .map_err(|_| TransportError::Limit)?;
    let snapshot = pb::Snapshot {
        revision: capture.state.revision,
        records: capture.records.clone(),
        required_features: vec!["cursor.vector.v1".into()],
        ..Default::default()
    }
    .encode_to_vec();
    let bytes = pb::Envelope {
        protocol_version: negotiated.version,
        session_id: negotiated.session_id.to_vec(),
        epoch: negotiated.epoch.to_vec(),
        view_id: negotiated.view_id.to_vec(),
        kind: 1,
        payload: snapshot,
        required_features: vec!["cursor.vector.v1".into()],
    }
    .encode_to_vec();
    validate_wire(&bytes, &model_limits(&negotiated.limits))?;
    // Core's allocation parent retains the boxed body/metadata; staging
    // accounts the pre-box Vec without a duplicate publication reservation.
    ctx.broker
        .snapshot_publication_payload(&negotiated.client, negotiated.view_id, bytes, None)
        .map_err(|_| TransportError::Limit)
}
fn snapshot(
    view: &mut View,
    native: &Connection,
    ctx: &mut Context<'_>,
) -> Result<SharedPayload, TransportError> {
    let source = ctx.backend.topology().snapshot().map_err(error)?;
    let view_memory = ctx
        .broker
        .reserve_view_memory(native.client(), native.view(), 2 << 20)
        .map_err(|_| TransportError::Limit)?;
    let (mut capture, _) =
        projection::next_with_limits(&source, view.capture.as_ref(), ctx.budget, &view.limits)
            .map_err(error)?;
    if view
        .capture
        .as_ref()
        .is_some_and(|old| old.state.revision == capture.state.revision)
    {
        // A reset checkpoint has a new exact publication body. Do not collide
        // with a retained transaction digest at the current revision, even if
        // source fields did not change since the last publication.
        capture.state.revision = capture
            .state
            .revision
            .checked_add(1)
            .ok_or(TransportError::Limit)?;
    }
    capture.charge_view(view_memory);
    let _staging = ctx
        .budget
        .reserve(1 << 20)
        .map_err(|_| TransportError::Limit)?;
    let bytes = envelope(
        native,
        1,
        pb::Snapshot {
            revision: capture.state.revision,
            records: capture.records.clone(),
            required_features: vec!["cursor.vector.v1".into()],
            ..Default::default()
        }
        .encode_to_vec(),
    );
    validate_wire(&bytes, &view.limits)?;
    // Prepare the retained body before any checkpoint mutation. The remaining
    // install_snapshot allocation either succeeds atomically or leaves the old
    // checkpoint intact, even if another parent-budget owner allocates now.
    let prepared = PreparedCheckpoint::new(ctx.broker, native.client(), native.view(), bytes)?;
    let payload = prepared.install(
        ctx.broker,
        native.client(),
        native.view(),
        capture.state.revision,
        capture.state.estimated_bytes(),
    )?;
    view.capture = Some(capture);
    view.checkpoint = Some(payload.clone());
    Ok(payload)
}
struct PreparedCheckpoint(SharedPayload);
impl PreparedCheckpoint {
    fn new(
        broker: &SessionBroker,
        client: &AuthenticatedClient,
        view: Id,
        bytes: Vec<u8>,
    ) -> Result<Self, TransportError> {
        broker
            .snapshot_publication_payload(client, view, bytes, None)
            .map(Self)
            .map_err(|_| TransportError::Limit)
    }
    fn install(
        self,
        broker: &mut SessionBroker,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
        decoded: usize,
    ) -> Result<SharedPayload, TransportError> {
        broker
            .install_snapshot(client, view, revision, decoded)
            .map_err(|_| TransportError::Limit)?;
        Ok(self.0)
    }
}
fn update(
    view: &mut View,
    native: &Connection,
    ctx: &mut Context<'_>,
) -> Result<Option<SharedPayload>, TransportError> {
    let old = view.capture.as_ref().ok_or(TransportError::Authority)?;
    if old.source_revision() == ctx.backend.topology().revision() {
        return Ok(None);
    }
    let source = ctx.backend.topology().snapshot().map_err(error)?;
    let view_memory = ctx
        .broker
        .reserve_view_memory(native.client(), native.view(), 2 << 20)
        .map_err(|_| TransportError::Limit)?;
    let (mut capture, mutations) =
        projection::next_with_limits(&source, Some(old), ctx.budget, &view.limits)
            .map_err(error)?;
    capture.charge_view(view_memory);
    if mutations.is_empty() {
        view.capture = Some(capture);
        return Ok(None);
    }
    let _staging = ctx
        .budget
        .reserve(1 << 20)
        .map_err(|_| TransportError::Limit)?;
    let bytes = envelope(
        native,
        2,
        pb::Transaction {
            base_revision: old.state.revision,
            new_revision: capture.state.revision,
            mutations,
            required_features: vec!["cursor.vector.v1".into()],
            ..Default::default()
        }
        .encode_to_vec(),
    );
    validate_wire(&bytes, &view.limits)?;
    ctx.broker
        .publish_with_attachment(
            native.client(),
            native.view(),
            old.state.revision,
            capture.state.revision,
            bytes,
            None,
            ctx.now,
        )
        .map_err(|_| TransportError::Authority)?;
    view.capture = Some(capture);
    ctx.broker
        .take_outbound(native.client(), native.view())
        .map_err(|_| TransportError::Authority)
}

#[cfg(test)]
mod allocation_tests {
    use super::*;
    #[test]
    fn parent_pressure_between_body_preparation_and_install_preserves_old_checkpoint() {
        let budget = MemoryBudget::new(4096);
        let config = SessionConfig::default();
        let mut broker =
            SessionBroker::with_memory_budget([1; 16], [2; 16], config.clone(), budget.clone())
                .unwrap();
        let negotiated = broker
            .negotiate(
                Some(VerifiedAuthorization {
                    principal: "scope".into(),
                    policy_version: 1,
                    permissions: rui_protocol_core::session::Permissions {
                        can_view: true,
                        can_control: false,
                        actions: Default::default(),
                    },
                }),
                ClientHello {
                    client_instance_id: [3; 16],
                    supported_versions: config.versions,
                    supported_profiles: config.supported_profiles,
                    required_profiles: config.required_profiles,
                    receiver_limits: config.receiver_limits,
                    resume: None,
                },
                0,
            )
            .unwrap();
        broker
            .install_snapshot(&negotiated.client, negotiated.view_id, 1, 100)
            .unwrap();
        let prepared = PreparedCheckpoint::new(
            &broker,
            &negotiated.client,
            negotiated.view_id,
            vec![7; 300],
        )
        .unwrap();
        let pressure = budget.reserve(budget.limit() - budget.used()).unwrap();
        assert!(matches!(
            prepared.install(&mut broker, &negotiated.client, negotiated.view_id, 2, 200),
            Err(TransportError::Limit)
        ));
        assert_eq!(
            broker
                .retained_memory_bytes(&negotiated.client, negotiated.view_id)
                .unwrap(),
            100
        );
        broker
            .acknowledge_applied(&negotiated.client, negotiated.view_id, 1, true)
            .unwrap();
        assert!(broker
            .acknowledge_applied(&negotiated.client, negotiated.view_id, 2, true)
            .is_err());
        drop(pressure);
        let prepared = PreparedCheckpoint::new(
            &broker,
            &negotiated.client,
            negotiated.view_id,
            vec![7; 300],
        )
        .unwrap();
        let frame = prepared
            .install(&mut broker, &negotiated.client, negotiated.view_id, 2, 200)
            .unwrap();
        broker
            .acknowledge_applied(&negotiated.client, negotiated.view_id, 2, true)
            .unwrap();
        drop(broker);
        assert_eq!(
            budget.used(),
            frame.len() + rui_protocol_core::session::PUBLICATION_OVERHEAD_BYTES
        );
        drop(frame);
        assert_eq!(budget.used(), 0);
    }
}
