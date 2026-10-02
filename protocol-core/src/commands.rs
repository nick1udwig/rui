//! At-most-once admission and semantic validation. Authentication and adapter
//! evidence are supplied by trusted host code; this module is not an authenticator.
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub type Id = [u8; 16];

/// A principal and producer identity already bound by the authentication layer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AuthenticatedClient {
    pub principal: String,
    pub client_instance_id: Id,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arguments {
    None,
    Checked(bool),
    Number(f64),
    ValueText(String),
    ReplaceText { start: u64, end: u64, text: String },
    Selection { anchor: u64, focus: u64 },
    Scroll { x: f64, y: f64 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preconditions {
    pub target_version: Option<u64>,
    pub text_version: Option<u64>,
    pub model_version: Option<u64>,
    pub focus_generation: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticCommand {
    pub epoch: Id,
    pub view_id: Id,
    pub client_instance_id: Id,
    pub command_seq: u64,
    pub control_lease_id: Id,
    pub control_generation: u64,
    pub target_id: u64,
    pub action_id: String,
    pub action_schema_version: u32,
    pub observed_view_revision: u64,
    pub preconditions: Preconditions,
    pub arguments: Arguments,
}

/// Trusted message-kind binding. The family is part of the retained exact key,
/// so identical protobuf payloads cannot be retried as a different operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandFamily {
    Semantic,
    RawInput,
    TextComposition,
    Resize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandHeader {
    pub epoch: Id,
    pub view_id: Id,
    pub client_instance_id: Id,
    pub command_seq: u64,
    pub control_lease_id: Id,
    pub control_generation: u64,
    pub observed_view_revision: u64,
}
impl SemanticCommand {
    pub fn header(&self) -> CommandHeader {
        CommandHeader {
            epoch: self.epoch,
            view_id: self.view_id,
            client_instance_id: self.client_instance_id,
            command_seq: self.command_seq,
            control_lease_id: self.control_lease_id,
            control_generation: self.control_generation,
            observed_view_revision: self.observed_view_revision,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyNamespace {
    LinuxEvdev,
}
#[derive(Clone, Debug)]
pub struct RawInputCommand {
    pub header: CommandHeader,
    pub seat_id: String,
    pub device_id: String,
    pub key_namespace: KeyNamespace,
    pub batch: crate::input::InputBatch,
}
#[derive(Clone, PartialEq, Eq)]
pub enum CompositionOperation {
    Begin(crate::input::ScalarRange),
    UpdatePreedit {
        text: String,
        selection: crate::input::ScalarRange,
    },
    CommitText {
        text: String,
    },
    Cancel,
}
#[derive(Clone)]
pub struct TextCompositionCommand {
    pub header: CommandHeader,
    pub target_id: u64,
    pub text_session_id: Id,
    pub focus_generation: u64,
    pub base_text_version: u64,
    pub operation: CompositionOperation,
}
#[derive(Clone, Debug)]
pub struct ResizeCommand {
    pub header: CommandHeader,
    pub surface_id: u64,
    pub surface_version: u64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone)]
pub enum Command {
    Semantic(SemanticCommand),
    RawInput(RawInputCommand),
    TextComposition(TextCompositionCommand),
    Resize(ResizeCommand),
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Command")
            .field("family", &self.family())
            .field("sequence", &self.header().command_seq)
            .finish()
    }
}
impl std::fmt::Debug for TextCompositionCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextCompositionCommand")
            .field("sequence", &self.header.command_seq)
            .field("operation", &"[REDACTED]")
            .finish()
    }
}
impl Command {
    pub fn family(&self) -> CommandFamily {
        match self {
            Self::Semantic(_) => CommandFamily::Semantic,
            Self::RawInput(_) => CommandFamily::RawInput,
            Self::TextComposition(_) => CommandFamily::TextComposition,
            Self::Resize(_) => CommandFamily::Resize,
        }
    }
    pub fn header(&self) -> CommandHeader {
        match self {
            Self::Semantic(c) => c.header(),
            Self::RawInput(c) => c.header,
            Self::TextComposition(c) => c.header,
            Self::Resize(c) => c.header,
        }
    }
    /// Exact operation permission from decoded authority-bearing bytes, never a
    /// caller-supplied string detached from the deduplication/dispatch payload.
    pub fn operation_permission(&self) -> &str {
        match self {
            Self::Semantic(c) => &c.action_id,
            Self::RawInput(_) => "RawInputBatch",
            Self::TextComposition(_) => "TextComposition",
            Self::Resize(_) => "ResizeRequest",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    Unauthorized,
    WrongView,
    NoControl,
    StaleControl,
    MissingTarget,
    DisabledTarget,
    UnsupportedAction,
    UnsupportedSchema,
    InvalidArguments,
    StaleTarget,
    StaleText,
    StaleModel,
    StaleFocus,
    FutureRevision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandStatus {
    Accepted,
    Dispatched,
    Applied,
    Rejected,
    Failed,
    OutcomeUnknown,
    OutcomeExpired,
}

/// Explicit completion evidence from a cooperating application, not inferred
/// from timestamps or a successful input-injection call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEvidence {
    source_commit: String,
    resulting_view_revision: Option<u64>,
}
impl SourceEvidence {
    pub fn new(
        source_commit: String,
        resulting_view_revision: Option<u64>,
    ) -> Result<Self, LedgerError> {
        if source_commit.is_empty()
            || source_commit.len() > 1024
            || resulting_view_revision == Some(0)
        {
            return Err(LedgerError::InvalidEvidence);
        }
        Ok(Self {
            source_commit,
            resulting_view_revision,
        })
    }
    pub fn source_commit(&self) -> &str {
        &self.source_commit
    }
    pub fn resulting_view_revision(&self) -> Option<u64> {
        self.resulting_view_revision
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub status: CommandStatus,
    pub rejection: Option<Rejection>,
    pub evidence: Option<SourceEvidence>,
}
impl CommandResult {
    fn status(status: CommandStatus) -> Self {
        Self {
            status,
            rejection: None,
            evidence: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    Dispatched,
    Applied(SourceEvidence),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchFailure {
    /// The adapter knows the operation failed without an ambiguous side effect.
    Failed,
    /// The adapter cannot establish whether an external effect occurred.
    OutcomeUnknown,
}

#[derive(Clone, Debug)]
pub struct LedgerLimits {
    pub max_clients: usize,
    pub max_commands: usize,
    pub max_retained_command_bytes: usize,
    pub max_command_bytes: usize,
    pub max_outcomes: usize,
    pub outcome_ttl_ms: u64,
}
impl Default for LedgerLimits {
    fn default() -> Self {
        Self {
            max_clients: 1024,
            max_commands: 65_536,
            max_retained_command_bytes: 64 * 1024 * 1024,
            max_command_bytes: 1024 * 1024,
            max_outcomes: 4096,
            outcome_ttl_ms: 60_000,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LedgerError {
    #[error("malformed or unsupported command")]
    Malformed,
    #[error("command exceeds receiver bound")]
    Oversized,
    #[error("command belongs to another authority epoch")]
    WrongEpoch,
    #[error("producer identity is not the authenticated registered client")]
    WrongClient,
    #[error("command sequence must be nonzero and contiguous")]
    SequenceGap,
    #[error("a consumed command key was reused with different exact bytes")]
    ChangedBytes,
    #[error("epoch ledger retention budget exhausted; restart with a new epoch, never evict high-water marks")]
    RetentionExhausted,
    #[error("command sequence was not consumed")]
    UnknownCommand,
    #[error("source evidence is missing or too large")]
    InvalidEvidence,
    #[error("invalid ledger limits")]
    InvalidLimits,
}

struct Record {
    family: CommandFamily,
    bytes: Vec<u8>,
    outcome: Option<CommandResult>,
    expires_at: u64,
    order: u64,
}
#[derive(Default)]
struct Producer {
    high_water: u64,
    records: BTreeMap<u64, Record>,
}

/// In-memory epoch ledger. Exact original bytes and high-water marks never
/// expire or get evicted. When their bounded budget fills, fresh commands are
/// refused. Only detailed outcomes expire. An authority losing this structure
/// must choose a new epoch before accepting any further commands.
pub struct CommandLedger {
    epoch: Id,
    limits: LedgerLimits,
    producers: BTreeMap<AuthenticatedClient, Producer>,
    retained_bytes: usize,
    command_count: usize,
    next_order: u64,
    last_now: u64,
}
impl CommandLedger {
    pub fn new(epoch: Id, limits: LedgerLimits) -> Result<Self, LedgerError> {
        if limits.max_clients == 0
            || limits.max_commands == 0
            || limits.max_retained_command_bytes == 0
            || limits.max_command_bytes == 0
            || limits.max_outcomes == 0
            || limits.outcome_ttl_ms == 0
        {
            return Err(LedgerError::InvalidLimits);
        }
        Ok(Self {
            epoch,
            limits,
            producers: BTreeMap::new(),
            retained_bytes: 0,
            command_count: 0,
            next_order: 0,
            last_now: 0,
        })
    }
    pub fn epoch(&self) -> Id {
        self.epoch
    }
    pub fn retained_command_bytes(&self) -> usize {
        self.retained_bytes
    }
    pub fn command_count(&self) -> usize {
        self.command_count
    }
    pub fn producer_count(&self) -> usize {
        self.producers.len()
    }
    pub fn high_water(&self, client: &AuthenticatedClient) -> u64 {
        self.producers.get(client).map_or(0, |p| p.high_water)
    }
    fn observe_time(&mut self, now: u64) -> u64 {
        self.last_now = self.last_now.max(now);
        self.last_now
    }
    pub fn expire_outcomes(&mut self, now: u64) {
        let now = self.observe_time(now);
        for producer in self.producers.values_mut() {
            for record in producer.records.values_mut() {
                if record.expires_at <= now {
                    record.outcome = None;
                }
            }
        }
    }
    fn bound_outcomes(&mut self) {
        let mut retained: Vec<_> = self
            .producers
            .iter()
            .flat_map(|(client, p)| {
                p.records
                    .iter()
                    .filter(|(_, r)| r.outcome.is_some())
                    .map(move |(seq, r)| (r.order, client.clone(), *seq))
            })
            .collect();
        retained.sort_by_key(|r| r.0);
        let remove = retained.len().saturating_sub(self.limits.max_outcomes);
        for (_, client, seq) in retained.into_iter().take(remove) {
            self.producers
                .get_mut(&client)
                .unwrap()
                .records
                .get_mut(&seq)
                .unwrap()
                .outcome = None;
        }
    }
    /// Decode the same original Protobuf payload used for exact-byte deduplication.
    /// No caller-provided decoded command can disagree with the dispatch key.
    pub fn submit<V, D>(
        &mut self,
        client: &AuthenticatedClient,
        raw: &[u8],
        now: u64,
        validate: V,
        dispatch: D,
    ) -> Result<CommandResult, LedgerError>
    where
        V: FnOnce(&SemanticCommand) -> Result<(), Rejection>,
        D: FnOnce(&SemanticCommand) -> Result<DispatchOutcome, DispatchFailure>,
    {
        self.submit_family(
            client,
            CommandFamily::Semantic,
            raw,
            now,
            |command| match command {
                Command::Semantic(command) => validate(command),
                _ => unreachable!(),
            },
            |command| match command {
                Command::Semantic(command) => dispatch(command),
                _ => unreachable!(),
            },
        )
    }
    /// Mandatory current session authority is checked inside reserved admission.
    /// The source validator still checks route/topology, focus, modality, target
    /// membership and relevant source clocks at the dispatch boundary. An exact
    /// retry returns its retained outcome without touching any adapter again.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_controlled<V, D>(
        &mut self,
        sessions: &mut crate::session::SessionBroker,
        client: &AuthenticatedClient,
        family: CommandFamily,
        raw: &[u8],
        now: u64,
        validate_source: V,
        dispatch: D,
    ) -> Result<CommandResult, LedgerError>
    where
        V: FnOnce(&Command) -> Result<(), Rejection>,
        D: FnOnce(&Command) -> Result<DispatchOutcome, DispatchFailure>,
    {
        self.submit_controlled_with_session(
            sessions,
            client,
            family,
            raw,
            now,
            validate_source,
            |command, _| dispatch(command),
        )
    }
    /// A cooperating source may publish its actual commit through this unique
    /// session reference before returning APPLIED evidence. Authority has already
    /// passed the same mandatory checks; callbacks remain trusted host code.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_controlled_with_session<V, D>(
        &mut self,
        sessions: &mut crate::session::SessionBroker,
        client: &AuthenticatedClient,
        family: CommandFamily,
        raw: &[u8],
        now: u64,
        validate_source: V,
        dispatch: D,
    ) -> Result<CommandResult, LedgerError>
    where
        V: FnOnce(&Command) -> Result<(), Rejection>,
        D: FnOnce(
            &Command,
            &mut crate::session::SessionBroker,
        ) -> Result<DispatchOutcome, DispatchFailure>,
    {
        let sessions = std::cell::RefCell::new(sessions);
        self.submit_family(
            client,
            family,
            raw,
            now,
            |command| {
                sessions.borrow_mut().validate_family_control(
                    client,
                    &command.header(),
                    command.family(),
                    command.operation_permission(),
                    now,
                )?;
                validate_source(command)
            },
            |command| dispatch(command, &mut sessions.borrow_mut()),
        )
    }
    fn submit_family<V, D>(
        &mut self,
        client: &AuthenticatedClient,
        family: CommandFamily,
        raw: &[u8],
        now: u64,
        validate: V,
        dispatch: D,
    ) -> Result<CommandResult, LedgerError>
    where
        V: FnOnce(&Command) -> Result<(), Rejection>,
        D: FnOnce(&Command) -> Result<DispatchOutcome, DispatchFailure>,
    {
        if raw.len() > self.limits.max_command_bytes {
            return Err(LedgerError::Oversized);
        }
        let command = decode_family_bounded(family, raw, self.limits.max_command_bytes)?;
        let header = command.header();
        if header.epoch != self.epoch {
            return Err(LedgerError::WrongEpoch);
        }
        if header.client_instance_id != client.client_instance_id
            || client.principal.is_empty()
            || client.principal.len() > 1024
        {
            return Err(LedgerError::WrongClient);
        }
        self.expire_outcomes(now);
        if let Some(producer) = self.producers.get(client) {
            if header.command_seq <= producer.high_water {
                let record = producer
                    .records
                    .get(&header.command_seq)
                    .ok_or(LedgerError::SequenceGap)?;
                if record.family != family || record.bytes != raw {
                    return Err(LedgerError::ChangedBytes);
                }
                return Ok(record
                    .outcome
                    .clone()
                    .unwrap_or_else(|| CommandResult::status(CommandStatus::OutcomeExpired)));
            }
        }
        let high_water = self.high_water(client);
        if high_water.checked_add(1) != Some(header.command_seq) {
            return Err(LedgerError::SequenceGap);
        }
        if (self.producers.len() >= self.limits.max_clients && !self.producers.contains_key(client))
            || self.command_count >= self.limits.max_commands
            || self
                .retained_bytes
                .checked_add(raw.len())
                .is_none_or(|n| n > self.limits.max_retained_command_bytes)
        {
            return Err(LedgerError::RetentionExhausted);
        }
        // Persist the consumed key and ambiguous outcome before invoking host code.
        // If validation/dispatch unwinds, this reservation still prevents redispatch.
        let order = self.next_order;
        self.next_order = self
            .next_order
            .checked_add(1)
            .ok_or(LedgerError::RetentionExhausted)?;
        let producer = self.producers.entry(client.clone()).or_default();
        producer.high_water = header.command_seq;
        producer.records.insert(
            header.command_seq,
            Record {
                family,
                bytes: raw.to_vec(),
                outcome: Some(CommandResult::status(CommandStatus::OutcomeUnknown)),
                expires_at: self.last_now.saturating_add(self.limits.outcome_ttl_ms),
                order,
            },
        );
        self.retained_bytes += raw.len();
        self.command_count += 1;
        self.bound_outcomes();
        let result = match validate(&command) {
            Err(rejection) => CommandResult {
                status: CommandStatus::Rejected,
                rejection: Some(rejection),
                evidence: None,
            },
            Ok(()) => match dispatch(&command) {
                Ok(DispatchOutcome::Dispatched) => CommandResult::status(CommandStatus::Dispatched),
                Ok(DispatchOutcome::Applied(evidence)) => CommandResult {
                    status: CommandStatus::Applied,
                    rejection: None,
                    evidence: Some(evidence),
                },
                Err(DispatchFailure::Failed) => CommandResult::status(CommandStatus::Failed),
                Err(DispatchFailure::OutcomeUnknown) => {
                    CommandResult::status(CommandStatus::OutcomeUnknown)
                }
            },
        };
        self.producers
            .get_mut(client)
            .unwrap()
            .records
            .get_mut(&header.command_seq)
            .unwrap()
            .outcome = Some(result.clone());
        self.bound_outcomes();
        Ok(result)
    }
    /// A read-only lookup: never consumes a sequence or invokes an adapter.
    pub fn get_result(
        &mut self,
        client: &AuthenticatedClient,
        epoch: Id,
        sequence: u64,
        now: u64,
    ) -> Result<CommandResult, LedgerError> {
        if epoch != self.epoch {
            return Err(LedgerError::WrongEpoch);
        }
        self.expire_outcomes(now);
        let record = self
            .producers
            .get(client)
            .and_then(|p| p.records.get(&sequence))
            .ok_or(LedgerError::UnknownCommand)?;
        Ok(record
            .outcome
            .clone()
            .unwrap_or_else(|| CommandResult::status(CommandStatus::OutcomeExpired)))
    }
}

fn id(raw: Vec<u8>) -> Result<Id, LedgerError> {
    raw.try_into().map_err(|_| LedgerError::Malformed)
}

pub fn decode_command(raw: &[u8]) -> Result<SemanticCommand, LedgerError> {
    if raw.len() > LedgerLimits::default().max_command_bytes {
        return Err(LedgerError::Oversized);
    }
    decode_bounded_command(raw)
}

fn decode_bounded_command(raw: &[u8]) -> Result<SemanticCommand, LedgerError> {
    decode_semantic(raw, LedgerLimits::default().max_command_bytes)
}
fn decode_semantic(raw: &[u8], maximum: usize) -> Result<SemanticCommand, LedgerError> {
    use crate::wire::proto::{semantic_command, value_arguments};
    let wire =
        crate::wire::decode_semantic_command(raw, &decode_limits(maximum)).map_err(decode_error)?;
    let arguments = match wire.arguments.ok_or(LedgerError::Malformed)? {
        semantic_command::Arguments::NoArguments(_) => Arguments::None,
        semantic_command::Arguments::SetValue(value) => {
            match value.value.ok_or(LedgerError::Malformed)? {
                value_arguments::Value::Checked(v) => Arguments::Checked(v),
                value_arguments::Value::Number(v) if v.is_finite() => Arguments::Number(v),
                value_arguments::Value::Text(v) => Arguments::ValueText(v),
                _ => return Err(LedgerError::Malformed),
            }
        }
        semantic_command::Arguments::ReplaceText(v) => Arguments::ReplaceText {
            start: v.start,
            end: v.end,
            text: v.text,
        },
        semantic_command::Arguments::SetSelection(v) => Arguments::Selection {
            anchor: v.anchor,
            focus: v.focus,
        },
        semantic_command::Arguments::ScrollTo(v) if v.x.is_finite() && v.y.is_finite() => {
            Arguments::Scroll { x: v.x, y: v.y }
        }
        _ => return Err(LedgerError::Malformed),
    };
    if wire.command_seq == 0
        || wire.target_id == 0
        || wire.action_id.is_empty()
        || wire.action_id.len() > 256
        || wire.action_schema_version == 0
        || wire.control_generation == 0
    {
        return Err(LedgerError::Malformed);
    }
    let preconditions = wire
        .preconditions
        .map(|p| Preconditions {
            target_version: p.target_version,
            text_version: p.text_version,
            model_version: p.model_version,
            focus_generation: p.focus_generation,
        })
        .unwrap_or_default();
    Ok(SemanticCommand {
        epoch: id(wire.epoch)?,
        view_id: id(wire.view_id)?,
        client_instance_id: id(wire.client_instance_id)?,
        command_seq: wire.command_seq,
        control_lease_id: id(wire.control_lease_id)?,
        control_generation: wire.control_generation,
        target_id: wire.target_id,
        action_id: wire.action_id,
        action_schema_version: wire.action_schema_version,
        observed_view_revision: wire.observed_view_revision,
        preconditions,
        arguments,
    })
}

fn decode_limits(maximum: usize) -> crate::model::Limits {
    crate::model::Limits {
        max_envelope_bytes: maximum,
        max_memory_bytes: maximum.saturating_mul(16).saturating_add(8192),
        max_records: 256,
        max_gradient_stops: 0,
        max_dash_segments: 0,
        max_string_bytes: maximum,
        ..crate::model::Limits::default()
    }
}
fn decode_error(error: crate::wire::WireError) -> LedgerError {
    match error {
        crate::wire::WireError::Limit => LedgerError::Oversized,
        _ => LedgerError::Malformed,
    }
}
fn header(wire: Option<crate::wire::proto::CommandHeader>) -> Result<CommandHeader, LedgerError> {
    let wire = wire.ok_or(LedgerError::Malformed)?;
    if wire.command_seq == 0 || wire.control_generation == 0 {
        return Err(LedgerError::Malformed);
    }
    Ok(CommandHeader {
        epoch: id(wire.epoch)?,
        view_id: id(wire.view_id)?,
        client_instance_id: id(wire.client_instance_id)?,
        command_seq: wire.command_seq,
        control_lease_id: id(wire.control_lease_id)?,
        control_generation: wire.control_generation,
        observed_view_revision: wire.observed_view_revision,
    })
}
fn point(wire: Option<crate::wire::proto::Point>) -> Result<crate::model::Point, LedgerError> {
    let wire = wire.ok_or(LedgerError::Malformed)?;
    if !wire.x.is_finite() || !wire.y.is_finite() {
        return Err(LedgerError::Malformed);
    }
    Ok(crate::model::Point {
        x: wire.x,
        y: wire.y,
    })
}
fn scalar_range(wire: crate::wire::proto::ScalarRange) -> crate::input::ScalarRange {
    crate::input::ScalarRange {
        start: wire.start,
        end: wire.end,
    }
}
/// Decode only the payload of its validated message kind. The ledger retains
/// that family binding alongside the original bytes; no decoded caller value
/// can substitute a different authority, operation, or sequence.
pub fn decode_family(family: CommandFamily, raw: &[u8]) -> Result<Command, LedgerError> {
    decode_family_bounded(family, raw, LedgerLimits::default().max_command_bytes)
}
fn decode_family_bounded(
    family: CommandFamily,
    raw: &[u8],
    maximum: usize,
) -> Result<Command, LedgerError> {
    use crate::input::{Event, InputBatch, TimedEvent};
    use crate::wire::proto::{raw_event, text_composition};
    if raw.len() > maximum {
        return Err(LedgerError::Oversized);
    }
    let limits = decode_limits(maximum);
    match family {
        CommandFamily::Semantic => Ok(Command::Semantic(decode_semantic(raw, maximum)?)),
        CommandFamily::RawInput => {
            let wire = crate::wire::decode_raw_input(raw, &limits).map_err(decode_error)?;
            if wire.surface_id == 0
                || wire.presented_revision == 0
                || wire.seat_id.is_empty()
                || wire.seat_id.len() > 256
                || wire.device_id.is_empty()
                || wire.device_id.len() > 256
                || wire.key_namespace != "linux.evdev"
                || wire.events.is_empty()
                || wire.events.len() > 256
            {
                return Err(LedgerError::Malformed);
            }
            let mut events = Vec::with_capacity(wire.events.len());
            for e in wire.events {
                let event = match e.event.ok_or(LedgerError::Malformed)? {
                    raw_event::Event::Motion(p) => Event::Motion(point(Some(p))?),
                    raw_event::Event::Button(b) => Event::Button {
                        code: b.code,
                        pressed: b.pressed,
                        position: point(b.position)?,
                    },
                    raw_event::Event::Key(k) => Event::Key {
                        code: k.code,
                        pressed: k.pressed,
                    },
                    raw_event::Event::Wheel(w) if w.x.is_finite() && w.y.is_finite() => {
                        Event::Wheel {
                            x: w.x,
                            y: w.y,
                            position: point(w.position)?,
                        }
                    }
                    raw_event::Event::TouchDown(t) => Event::TouchDown {
                        id: t.id,
                        position: point(t.position)?,
                    },
                    raw_event::Event::TouchMove(t) => Event::TouchMove {
                        id: t.id,
                        position: point(t.position)?,
                    },
                    raw_event::Event::TouchUp(t) => Event::TouchUp { id: t.id },
                    raw_event::Event::TouchCancel(t) => Event::TouchCancel { id: t.id },
                    _ => return Err(LedgerError::Malformed),
                };
                events.push(TimedEvent {
                    timestamp_us: e.timestamp_us,
                    event,
                });
            }
            Ok(Command::RawInput(RawInputCommand {
                header: header(wire.header)?,
                seat_id: wire.seat_id,
                device_id: wire.device_id,
                key_namespace: KeyNamespace::LinuxEvdev,
                batch: InputBatch {
                    surface: wire.surface_id,
                    routing_token: id(wire.routing_token)?,
                    presented_revision: wire.presented_revision,
                    events,
                },
            }))
        }
        CommandFamily::TextComposition => {
            let wire = crate::wire::decode_text_composition(raw, &limits).map_err(decode_error)?;
            if wire.target_id == 0 || wire.focus_generation == 0 || wire.base_text_version == 0 {
                return Err(LedgerError::Malformed);
            }
            let operation = match wire.operation.ok_or(LedgerError::Malformed)? {
                text_composition::Operation::Begin(r) => {
                    CompositionOperation::Begin(scalar_range(r))
                }
                text_composition::Operation::UpdatePreedit(p) => {
                    CompositionOperation::UpdatePreedit {
                        text: p.text,
                        selection: scalar_range(p.selection.ok_or(LedgerError::Malformed)?),
                    }
                }
                text_composition::Operation::CommitText(c) => {
                    CompositionOperation::CommitText { text: c.text }
                }
                text_composition::Operation::Cancel(_) => CompositionOperation::Cancel,
            };
            Ok(Command::TextComposition(TextCompositionCommand {
                header: header(wire.header)?,
                target_id: wire.target_id,
                text_session_id: id(wire.text_session_id)?,
                focus_generation: wire.focus_generation,
                base_text_version: wire.base_text_version,
                operation,
            }))
        }
        CommandFamily::Resize => {
            let wire = crate::wire::decode_resize_request(raw, &limits).map_err(decode_error)?;
            if wire.surface_id == 0
                || wire.surface_version == 0
                || !wire.width.is_finite()
                || !wire.height.is_finite()
            {
                return Err(LedgerError::Malformed);
            }
            Ok(Command::Resize(ResizeCommand {
                header: header(wire.header)?,
                surface_id: wire.surface_id,
                surface_version: wire.surface_version,
                width: wire.width,
                height: wire.height,
            }))
        }
    }
}

#[derive(Clone, Debug)]
pub enum ValueSchema {
    Boolean,
    Number { min: f64, max: f64 },
    Text { max_scalars: usize },
}

#[derive(Clone, Debug)]
pub struct TargetState {
    pub enabled: bool,
    pub target_version: u64,
    pub text_version: Option<u64>,
    pub model_version: Option<u64>,
    pub focus_generation: Option<u64>,
    pub text_scalar_count: Option<u64>,
    pub value_schema: Option<ValueSchema>,
    /// Standard action name -> exact supported schema version.
    pub actions: BTreeMap<String, u32>,
}

/// A dispatch-time projection supplied by the authority, never by a client.
#[derive(Clone, Debug)]
pub struct SemanticAuthority {
    pub view_id: Id,
    pub current_view_revision: u64,
    pub allowed_actions: BTreeSet<String>,
    pub targets: BTreeMap<u64, TargetState>,
}
impl SemanticAuthority {
    /// Overall older view revisions are context, not a whole-view compare-and-swap.
    /// Call session::SessionBroker::validate_control before this validation.
    pub fn validate(&self, command: &SemanticCommand) -> Result<(), Rejection> {
        if command.view_id != self.view_id {
            return Err(Rejection::WrongView);
        }
        if !self.allowed_actions.contains(&command.action_id) {
            return Err(Rejection::Unauthorized);
        }
        if command.observed_view_revision > self.current_view_revision {
            return Err(Rejection::FutureRevision);
        }
        let target = self
            .targets
            .get(&command.target_id)
            .ok_or(Rejection::MissingTarget)?;
        if !target.enabled {
            return Err(Rejection::DisabledTarget);
        }
        let schema = target
            .actions
            .get(&command.action_id)
            .ok_or(Rejection::UnsupportedAction)?;
        if *schema != command.action_schema_version {
            return Err(Rejection::UnsupportedSchema);
        }
        let p = &command.preconditions;
        if p.target_version.is_some_and(|v| v != target.target_version) {
            return Err(Rejection::StaleTarget);
        }
        if p.text_version
            .is_some_and(|v| target.text_version != Some(v))
        {
            return Err(Rejection::StaleText);
        }
        if p.model_version
            .is_some_and(|v| target.model_version != Some(v))
        {
            return Err(Rejection::StaleModel);
        }
        if p.focus_generation
            .is_some_and(|v| target.focus_generation != Some(v))
        {
            return Err(Rejection::StaleFocus);
        }
        match (command.action_id.as_str(), &command.arguments) {
            ("Activate" | "Focus", Arguments::None) => {}
            ("SetValue", value) => {
                let valid = match (&target.value_schema, value) {
                    (Some(ValueSchema::Boolean), Arguments::Checked(_)) => true,
                    (Some(ValueSchema::Number { min, max }), Arguments::Number(value)) => {
                        value.is_finite()
                            && min.is_finite()
                            && max.is_finite()
                            && min <= max
                            && value >= min
                            && value <= max
                    }
                    (Some(ValueSchema::Text { max_scalars }), Arguments::ValueText(text)) => {
                        text.chars().count() <= *max_scalars
                    }
                    _ => false,
                };
                if !valid {
                    return Err(Rejection::InvalidArguments);
                }
            }
            ("ReplaceText", Arguments::ReplaceText { start, end, .. }) => {
                if p.text_version.is_none()
                    || target.text_scalar_count.is_none()
                    || start > end
                    || *end > target.text_scalar_count.unwrap()
                {
                    return Err(Rejection::InvalidArguments);
                }
            }
            ("SetSelection", Arguments::Selection { anchor, focus }) => {
                if p.text_version.is_none()
                    || target.text_scalar_count.is_none()
                    || *anchor > target.text_scalar_count.unwrap()
                    || *focus > target.text_scalar_count.unwrap()
                {
                    return Err(Rejection::InvalidArguments);
                }
            }
            ("ScrollTo", Arguments::Scroll { x, y }) if x.is_finite() && y.is_finite() => {}
            _ => return Err(Rejection::InvalidArguments),
        }
        Ok(())
    }
}

#[cfg(test)]
mod family_tests {
    use super::*;
    use prost::Message;
    #[test]
    fn retained_family_binding_is_checked_even_when_exact_bytes_match() {
        let client = AuthenticatedClient {
            principal: "controller".into(),
            client_instance_id: [3; 16],
        };
        let wire = crate::wire::proto::SemanticCommand {
            epoch: vec![1; 16],
            view_id: vec![2; 16],
            client_instance_id: vec![3; 16],
            command_seq: 1,
            control_lease_id: vec![4; 16],
            control_generation: 1,
            target_id: 1,
            action_id: "Activate".into(),
            action_schema_version: 1,
            observed_view_revision: 1,
            preconditions: None,
            arguments: Some(
                crate::wire::proto::semantic_command::Arguments::NoArguments(
                    crate::wire::proto::EmptyArguments {},
                ),
            ),
        };
        let raw = wire.encode_to_vec();
        let mut ledger = CommandLedger::new([1; 16], LedgerLimits::default()).unwrap();
        ledger
            .submit(
                &client,
                &raw,
                0,
                |_| Ok(()),
                |_| Ok(DispatchOutcome::Dispatched),
            )
            .unwrap();
        // Exercise the retained discriminator directly: equal bytes alone never
        // authorize a result lookup under a different interpreted family.
        ledger
            .producers
            .get_mut(&client)
            .unwrap()
            .records
            .get_mut(&1)
            .unwrap()
            .family = CommandFamily::Resize;
        assert_eq!(
            ledger.submit(&client, &raw, 0, |_| panic!(), |_| panic!()),
            Err(LedgerError::ChangedBytes)
        );
    }
}
