//! Bounded binary wire conversion. Command payload bytes are passed unchanged to
//! the host ledger; publication/resource hashes cover the original payload.
use crate::{proto as p, TransportError};
use prost::Message;
use rui_protocol_core::commands::{CommandResult, Id};
use rui_protocol_core::publication::{PublicationBegin, PublicationCommit, PublicationKind};
use rui_protocol_core::resources::{ResourceKind, ResourceOffer, ResourceProvenance};
use rui_protocol_core::session::{ClientHello, ReceiverLimits, ResumeRequest};
use std::collections::BTreeSet;

pub fn id(raw: &[u8]) -> Result<Id, TransportError> {
    raw.try_into().map_err(|_| TransportError::Protocol)
}
pub fn digest(raw: &[u8]) -> Result<[u8; 32], TransportError> {
    raw.try_into().map_err(|_| TransportError::Protocol)
}
fn varint(raw: &[u8], cursor: &mut usize) -> Result<u64, TransportError> {
    let mut value = 0;
    for shift in (0..70).step_by(7) {
        let byte = *raw.get(*cursor).ok_or(TransportError::Protocol)?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            return Err(TransportError::Protocol);
        }
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(TransportError::Protocol)
}
/// Every network message has a sealed schema. Preflight recursively inspects
/// merged submessages and packed fields before prost allocates. The fixed-size
/// scanner charges a conservative allocation bound of 4 * wire ceiling + 4096;
/// byte strings are opaque only when their own bounded decoder follows.
pub trait WireMessage: Message + Default + sealed::Schema {}
mod sealed {
    pub trait Schema {
        const SCHEMA: super::Schema;
    }
}
#[derive(Clone, Copy)]
pub enum Schema {
    Packet,
    Limits,
    ClientHello,
    Permissions,
    ServerHello,
    OpenView,
    ViewOpened,
    RevisionAck,
    ResumeRequest,
    ResumeAccepted,
    ResetRequired,
    AcquireControl,
    RenewControl,
    ReleaseControl,
    ControlGranted,
    ControlRevoked,
    GetCommandResult,
    CommandResult,
    PublicationBegin,
    PublicationChunk,
    PublicationCommit,
    AbortPublication,
    ResourceOffer,
    ResourceIds,
    ResourceChunk,
    ResourceComplete,
    Error,
    Ping,
}
macro_rules! schemas {($($name:ident),+)=>{$(impl sealed::Schema for p::$name { const SCHEMA: Schema=Schema::$name; } impl WireMessage for p::$name {})+};}
schemas!(
    Packet,
    Limits,
    ClientHello,
    Permissions,
    ServerHello,
    OpenView,
    ViewOpened,
    RevisionAck,
    ResumeRequest,
    ResumeAccepted,
    ResetRequired,
    AcquireControl,
    RenewControl,
    ReleaseControl,
    ControlGranted,
    ControlRevoked,
    GetCommandResult,
    CommandResult,
    PublicationBegin,
    PublicationChunk,
    PublicationCommit,
    AbortPublication,
    ResourceOffer,
    ResourceIds,
    ResourceChunk,
    ResourceComplete,
    Error,
    Ping
);
#[derive(Clone, Copy)]
enum Field {
    Scalar,
    Fixed64,
    Bytes(usize),
    String(usize),
    Strings,
    Digests,
    Versions,
    Nested(Schema),
}
impl Schema {
    fn field(self, n: usize) -> Option<Field> {
        use Field::*;
        use Schema as S;
        let id = Bytes(16);
        let hash = Bytes(32);
        match (self, n) {
            (S::Packet, 1..=3) => Some(id),
            (S::Packet, 4..=5) => Some(Scalar),
            (S::Packet, 6) => Some(Bytes(1024 * 1024)),
            (S::Packet, 7) => Some(Strings),
            (S::Limits, 1..=9) => Some(Scalar),
            (S::ClientHello, 1) => Some(hash),
            (S::ClientHello, 2) => Some(id),
            (S::ClientHello, 3) => Some(Versions),
            (S::ClientHello, 4..=5) => Some(Strings),
            (S::ClientHello, 6) => Some(Nested(S::Limits)),
            (S::ClientHello, 7) => Some(Nested(S::ResumeRequest)),
            (S::ClientHello, 8) => Some(Scalar),
            (S::Permissions, 1..=2) => Some(Scalar),
            (S::Permissions, 3) => Some(Strings),
            (S::ServerHello, 1) => Some(Scalar),
            (S::ServerHello, 2) => Some(Strings),
            (S::ServerHello, 3) => Some(Nested(S::Limits)),
            (S::ServerHello, 4) => Some(Nested(S::Permissions)),
            (S::ServerHello, 5) => Some(id),
            (S::OpenView, 1..=2) => Some(String(256)),
            (S::ViewOpened, 1) => Some(id),
            (S::ViewOpened, 2..=3) => Some(Fixed64),
            (S::ViewOpened, 4) => Some(String(256)),
            (S::ViewOpened, 5) => Some(Strings),
            (S::RevisionAck, 1) | (S::GetCommandResult, 1) | (S::Ping, 1) => Some(Scalar),
            (S::ResumeRequest, 1..=2) => Some(id),
            (S::ResumeRequest, 3) | (S::ResumeAccepted, 1..=3) => Some(Scalar),
            (S::ResetRequired, 1) | (S::Error, 1) => Some(String(256)),
            (S::AcquireControl, 1) | (S::RenewControl, 1..=2) | (S::ReleaseControl, 1..=2) => {
                Some(id)
            }
            (S::RenewControl, 3) | (S::ReleaseControl, 3) => Some(Scalar),
            (S::ControlGranted, 1) | (S::ControlGranted, 5) => Some(id),
            (S::ControlGranted, 2..=3) => Some(Scalar),
            (S::ControlGranted, 4) => Some(Strings),
            (S::ControlRevoked, 1) => Some(id),
            (S::ControlRevoked, 2) => Some(Scalar),
            (S::ControlRevoked, 3) => Some(String(256)),
            (S::CommandResult, 1) | (S::CommandResult, 5) => Some(Scalar),
            (S::CommandResult, 2..=3) => Some(String(256)),
            (S::CommandResult, 4) => Some(String(1024)),
            (S::PublicationBegin, 1) => Some(id),
            (S::PublicationBegin, 2..=6) => Some(Scalar),
            (S::PublicationBegin, 7) => Some(hash),
            (S::PublicationChunk, 1) => Some(id),
            (S::PublicationChunk, 2) => Some(Scalar),
            (S::PublicationChunk, 3) => Some(Bytes(1024 * 1024)),
            (S::PublicationCommit, 1) => Some(id),
            (S::PublicationCommit, 2..=4) => Some(Scalar),
            (S::PublicationCommit, 5) => Some(hash),
            (S::AbortPublication, 1) => Some(id),
            (S::ResourceOffer, 1) => Some(hash),
            (S::ResourceOffer, 2..=6) => Some(Scalar),
            (S::ResourceIds, 1) => Some(Digests),
            (S::ResourceChunk, 1) => Some(hash),
            (S::ResourceChunk, 2) => Some(Scalar),
            (S::ResourceChunk, 3) => Some(Bytes(1024 * 1024)),
            (S::ResourceComplete, 1) => Some(hash),
            _ => None,
        }
    }
}
struct Scan {
    counts: [[u16; 16]; 28],
    fields: usize,
    allocation: usize,
    budget: usize,
}
impl Scan {
    fn charge(&mut self, n: usize) -> Result<(), TransportError> {
        self.allocation = self
            .allocation
            .checked_add(n)
            .filter(|n| *n <= self.budget)
            .ok_or(TransportError::Limit)?;
        Ok(())
    }
    fn count(&mut self, schema: Schema, field: usize, maximum: u16) -> Result<(), TransportError> {
        let count = &mut self.counts[schema as usize][field];
        *count = count
            .checked_add(1)
            .filter(|n| *n <= maximum)
            .ok_or(TransportError::Limit)?;
        Ok(())
    }
    fn message(&mut self, raw: &[u8], schema: Schema, depth: u8) -> Result<(), TransportError> {
        if depth > 8 {
            return Err(TransportError::Limit);
        }
        let mut cursor = 0;
        while cursor < raw.len() {
            self.fields += 1;
            if self.fields > 4096 {
                return Err(TransportError::Limit);
            }
            let key = varint(raw, &mut cursor)?;
            let field = usize::try_from(key >> 3).map_err(|_| TransportError::Protocol)?;
            let wire = key & 7;
            if field == 0 {
                return Err(TransportError::Protocol);
            }
            let rule = schema.field(field);
            if rule.is_none() && field < 128 {
                return Err(TransportError::Protocol);
            }
            let body = match wire {
                0 => {
                    varint(raw, &mut cursor)?;
                    &raw[0..0]
                }
                1 | 2 | 5 => {
                    let length = match wire {
                        1 => 8,
                        5 => 4,
                        _ => usize::try_from(varint(raw, &mut cursor)?)
                            .map_err(|_| TransportError::Limit)?,
                    };
                    let end = cursor
                        .checked_add(length)
                        .filter(|n| *n <= raw.len())
                        .ok_or(TransportError::Protocol)?;
                    let body = &raw[cursor..end];
                    cursor = end;
                    body
                }
                _ => return Err(TransportError::Protocol),
            };
            match rule {
                None => {}
                Some(Field::Scalar) if wire == 0 => self.count(schema, field, 64)?,
                Some(Field::Fixed64) if wire == 1 => self.count(schema, field, 64)?,
                Some(Field::Bytes(maximum) | Field::String(maximum)) if wire == 2 => {
                    self.count(schema, field, 64)?;
                    if body.len() > maximum {
                        return Err(TransportError::Limit);
                    }
                    self.charge(24 + body.len())?;
                }
                Some(Field::Strings | Field::Digests) if wire == 2 => {
                    let maximum = if matches!(rule, Some(Field::Digests)) {
                        32
                    } else {
                        256
                    };
                    self.count(schema, field, 128)?;
                    if body.len() > maximum {
                        return Err(TransportError::Limit);
                    }
                    self.charge(24 + body.len())?;
                }
                Some(Field::Versions) if wire == 0 => {
                    self.count(schema, field, 128)?;
                    self.charge(4)?;
                }
                Some(Field::Versions) if wire == 2 => {
                    let mut c = 0;
                    while c < body.len() {
                        varint(body, &mut c)?;
                        self.count(schema, field, 128)?;
                        self.charge(4)?;
                    }
                }
                Some(Field::Nested(child)) if wire == 2 => {
                    self.count(schema, field, 64)?;
                    self.charge(256)?;
                    self.message(body, child, depth + 1)?;
                }
                _ => return Err(TransportError::Protocol),
            }
        }
        Ok(())
    }
}
pub fn decode<T: WireMessage>(raw: &[u8], ceiling: usize) -> Result<T, TransportError> {
    if ceiling > 1024 * 1024 || raw.len() > ceiling {
        return Err(TransportError::Limit);
    }
    let mut scan = Scan {
        counts: [[0; 16]; 28],
        fields: 0,
        allocation: std::mem::size_of::<T>(),
        budget: ceiling.saturating_mul(4).saturating_add(4096),
    };
    scan.message(raw, T::SCHEMA, 0)?;
    T::decode(raw).map_err(|_| TransportError::Protocol)
}
pub fn packet(
    raw: &[u8],
    ceiling: usize,
    binding: Option<(Id, Id, Id)>,
) -> Result<p::Packet, TransportError> {
    let packet: p::Packet = decode(raw, ceiling)?;
    p::Kind::try_from(packet.kind).map_err(|_| TransportError::Protocol)?;
    if packet.kind == p::Kind::Unspecified as i32 || packet.protocol_version != 1 {
        return Err(TransportError::Protocol);
    }
    if packet.required_features.len() > 128
        || packet.required_features.iter().any(|f| f.len() > 256)
    {
        return Err(TransportError::Limit);
    }
    rui_protocol_core::wire::required_features(&packet.required_features)
        .map_err(|_| TransportError::Protocol)?;
    if let Some((session, epoch, view)) = binding {
        if id(&packet.session_id)? != session
            || id(&packet.epoch)? != epoch
            || id(&packet.view_id)? != view
        {
            return Err(TransportError::Binding);
        }
    } else if !(packet.session_id.is_empty()
        && packet.epoch.is_empty()
        && packet.view_id.is_empty())
    {
        return Err(TransportError::Binding);
    }
    Ok(packet)
}
pub fn wrap<T: Message>(kind: p::Kind, payload: &T, binding: Option<(Id, Id, Id)>) -> p::Packet {
    wrap_bytes(kind, payload.encode_to_vec(), binding)
}
pub fn wrap_bytes(kind: p::Kind, payload: Vec<u8>, binding: Option<(Id, Id, Id)>) -> p::Packet {
    let (session_id, epoch, view_id) = binding
        .map(|(s, e, v)| (s.to_vec(), e.to_vec(), v.to_vec()))
        .unwrap_or_default();
    p::Packet {
        session_id,
        epoch,
        view_id,
        protocol_version: 1,
        kind: kind as i32,
        payload,
        required_features: vec![],
    }
}
pub fn wire_limits(limits: &ReceiverLimits) -> p::Limits {
    p::Limits {
        envelope_bytes: limits.envelope_bytes as u64,
        chunk_bytes: limits.chunk_bytes as u64,
        transaction_bytes: limits.transaction_bytes as u64,
        snapshot_bytes: limits.snapshot_bytes as u64,
        resource_bytes: limits.resource_bytes as u64,
        view_memory_bytes: limits.view_memory_bytes as u64,
        outbound_queue_bytes: limits.outbound_queue_bytes as u64,
        records: limits.records as u64,
        ownership_depth: limits.ownership_depth as u64,
    }
}
pub fn limits(wire: p::Limits) -> Result<ReceiverLimits, TransportError> {
    let n = |v| usize::try_from(v).map_err(|_| TransportError::Limit);
    let limits = ReceiverLimits {
        envelope_bytes: n(wire.envelope_bytes)?,
        chunk_bytes: n(wire.chunk_bytes)?,
        transaction_bytes: n(wire.transaction_bytes)?,
        snapshot_bytes: n(wire.snapshot_bytes)?,
        resource_bytes: n(wire.resource_bytes)?,
        view_memory_bytes: n(wire.view_memory_bytes)?,
        outbound_queue_bytes: n(wire.outbound_queue_bytes)?,
        records: n(wire.records)?,
        ownership_depth: n(wire.ownership_depth)?,
    };
    limits
        .negotiate(&limits)
        .map_err(|_| TransportError::Limit)?;
    Ok(limits)
}
pub fn hello(mut wire: p::ClientHello) -> Result<ClientHello, TransportError> {
    let strings = |v: Vec<String>| -> Result<BTreeSet<String>, TransportError> {
        if v.len() > 128 || v.iter().any(|s| s.is_empty() || s.len() > 256) {
            return Err(TransportError::Limit);
        }
        Ok(v.into_iter().collect())
    };
    if wire.supported_versions.len() > 128 {
        return Err(TransportError::Limit);
    }
    Ok(ClientHello {
        client_instance_id: id(&wire.client_instance_id)?,
        supported_versions: std::mem::take(&mut wire.supported_versions)
            .into_iter()
            .collect(),
        supported_profiles: strings(std::mem::take(&mut wire.supported_profiles))?,
        required_profiles: strings(std::mem::take(&mut wire.required_profiles))?,
        receiver_limits: limits(
            wire.receiver_limits
                .take()
                .ok_or(TransportError::Protocol)?,
        )?,
        resume: wire
            .resume
            .take()
            .map(|r| {
                Ok(ResumeRequest {
                    epoch: id(&r.epoch)?,
                    view_id: id(&r.view_id)?,
                    acknowledged_revision: r.acknowledged_revision,
                })
            })
            .transpose()?,
    })
}
pub fn command_result(sequence: u64, result: CommandResult) -> p::CommandResult {
    p::CommandResult {
        command_seq: sequence,
        status: format!("{:?}", result.status),
        rejection: result
            .rejection
            .map(|r| format!("{r:?}"))
            .unwrap_or_default(),
        source_commit: result
            .evidence
            .as_ref()
            .map(|e| e.source_commit().to_owned())
            .unwrap_or_default(),
        resulting_view_revision: result.evidence.and_then(|e| e.resulting_view_revision()),
    }
}
pub fn publication_begin(
    kind: p::Kind,
    wire: p::PublicationBegin,
) -> Result<PublicationBegin, TransportError> {
    let kind = match kind {
        p::Kind::SnapshotBegin if wire.base_revision == 0 => PublicationKind::Snapshot {
            revision: wire.revision,
        },
        p::Kind::TransactionBegin => PublicationKind::Transaction {
            base_revision: wire.base_revision,
            new_revision: wire.revision,
        },
        _ => return Err(TransportError::Protocol),
    };
    Ok(PublicationBegin {
        publication_id: id(&wire.publication_id)?,
        kind,
        total_bytes: wire.total_bytes,
        chunk_count: wire.chunk_count,
        item_count: wire.item_count,
        digest: digest(&wire.digest)?,
    })
}
pub fn publication_commit(wire: p::PublicationCommit) -> Result<PublicationCommit, TransportError> {
    Ok(PublicationCommit {
        total_bytes: wire.total_bytes,
        chunk_count: wire.chunk_count,
        item_count: wire.item_count,
        digest: digest(&wire.digest)?,
    })
}
pub fn resource_offer(wire: p::ResourceOffer) -> Result<ResourceOffer, TransportError> {
    Ok(ResourceOffer {
        kind: ResourceKind::try_from(wire.kind).map_err(|_| TransportError::Protocol)?,
        version: wire.format_version,
        total_bytes: wire.total_bytes,
        provenance: ResourceProvenance::try_from(wire.provenance)
            .map_err(|_| TransportError::Protocol)?,
        digest: digest(&wire.digest)?,
    })
}
pub fn wire_resource_offer(offer: &ResourceOffer, chunk_count: u32) -> p::ResourceOffer {
    p::ResourceOffer {
        kind: offer.kind as u32,
        format_version: offer.version,
        total_bytes: offer.total_bytes,
        provenance: offer.provenance as u32,
        digest: offer.digest.to_vec(),
        chunk_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recursively_counts_merged_permissions_before_allocating() {
        // Each singular Permissions occurrence merges into the same prost value.
        let mut raw = Vec::new();
        for _ in 0..43 {
            raw.extend_from_slice(&[34, 6, 26, 0, 26, 0, 26, 0]);
        }
        assert_eq!(
            decode::<p::ServerHello>(&raw, 4096),
            Err(TransportError::Limit)
        );
        let mut hello = Vec::new();
        for _ in 0..65 {
            hello.extend_from_slice(&[50, 0]);
        }
        assert_eq!(
            decode::<p::ClientHello>(&hello, 4096),
            Err(TransportError::Limit)
        );
    }
    #[test]
    fn profiles_and_packed_versions_have_predecode_bounds() {
        let raw = [34, 0].repeat(129);
        assert_eq!(
            decode::<p::ClientHello>(&raw, 4096),
            Err(TransportError::Limit)
        );
        let mut packed = vec![26, 129, 1];
        packed.extend([1; 129]);
        assert_eq!(
            decode::<p::ClientHello>(&packed, 4096),
            Err(TransportError::Limit)
        );
        assert_eq!(
            decode::<p::Packet>(
                &[10, 17, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                512
            ),
            Err(TransportError::Limit)
        );
        assert_eq!(
            decode::<p::Ping>(&[18, 0], 512),
            Err(TransportError::Protocol)
        );
        // Unknown optional fields are skipped, while required vocabulary stays strict.
        assert_eq!(decode::<p::Ping>(&[128, 8, 1], 512).unwrap().nonce, 0);
    }
    #[test]
    fn small_envelopes_bound_decoded_metadata_and_debug_redacts() {
        let permissions = p::Permissions {
            can_view: true,
            can_control: true,
            actions: vec![String::new(); 128],
        };
        let hello = p::ServerHello {
            selected_profiles: vec![String::new(); 128],
            permissions: Some(permissions),
            ..Default::default()
        };
        assert_eq!(
            decode::<p::ServerHello>(&hello.encode_to_vec(), 512),
            Err(TransportError::Limit)
        );
        let secret = "credential-and-private-command";
        let hello = p::ClientHello {
            credential: secret.as_bytes().to_vec(),
            client_instance_id: vec![],
            supported_versions: vec![],
            supported_profiles: vec![],
            required_profiles: vec![],
            receiver_limits: None,
            resume: None,
            fresh_view: false,
        };
        assert!(!format!("{hello:?}").contains(secret));
        let packet = wrap(p::Kind::ClientHello, &hello, None);
        assert!(!format!("{packet:?}").contains(secret));
        assert_eq!(log::STATIC_MAX_LEVEL, log::LevelFilter::Off);
        log::trace!("plaintext must never be emitted: {secret}");
    }
}
