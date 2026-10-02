//! Bounded Protobuf decoding. Integrity hashes always cover received bytes.
use crate::model::Limits;
use prost::Message;
use thiserror::Error;

#[allow(clippy::large_enum_variant)]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/rui.v1.rs"));
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    #[error("wire or decoded allocation limit exceeded")]
    Limit,
    #[error("invalid protobuf: {0}")]
    Malformed(String),
    #[error("unsupported required feature: {0}")]
    Unsupported(String),
    #[error("invalid envelope binding, version, or message kind")]
    Envelope,
}

pub const SUPPORTED_FEATURES: &[&str] = &[
    "content.no-raster.v1",
    "mirror2d.core.v1",
    "semantics.core.v1",
];

pub fn required_features(features: &[String]) -> Result<(), WireError> {
    for feature in features {
        if !SUPPORTED_FEATURES.contains(&feature.as_str()) {
            return Err(WireError::Unsupported(feature.clone()));
        }
    }
    Ok(())
}

pub fn decode_envelope(bytes: &[u8], limits: &Limits) -> Result<proto::Envelope, WireError> {
    preflight(bytes, Context::Envelope, limits.max_envelope_bytes, limits)?;
    let envelope = proto::Envelope::decode(bytes).map_err(decode_error)?;
    if envelope.protocol_version != 1
        || envelope.session_id.len() != 16
        || envelope.epoch.len() != 16
        || envelope.view_id.len() != 16
        || !matches!(
            proto::MessageKind::try_from(envelope.kind),
            Ok(proto::MessageKind::Snapshot
                | proto::MessageKind::Transaction
                | proto::MessageKind::SemanticCommand)
        )
    {
        return Err(WireError::Envelope);
    }
    required_features(&envelope.required_features)?;
    Ok(envelope)
}

/// Checks an envelope against the authenticated connection and authorized view.
pub fn decode_bound_envelope(
    bytes: &[u8],
    limits: &Limits,
    session: &[u8; 16],
    epoch: &[u8; 16],
    view: &[u8; 16],
) -> Result<proto::Envelope, WireError> {
    let envelope = decode_envelope(bytes, limits)?;
    if envelope.session_id != session || envelope.epoch != epoch || envelope.view_id != view {
        return Err(WireError::Envelope);
    }
    Ok(envelope)
}

pub fn decode_snapshot(bytes: &[u8], limits: &Limits) -> Result<proto::Snapshot, WireError> {
    preflight(bytes, Context::Snapshot, limits.max_snapshot_bytes, limits)?;
    let result = proto::Snapshot::decode(bytes).map_err(decode_error)?;
    required_features(&result.required_features)?;
    Ok(result)
}

pub fn decode_transaction(bytes: &[u8], limits: &Limits) -> Result<proto::Transaction, WireError> {
    preflight(
        bytes,
        Context::Transaction,
        limits.max_transaction_bytes,
        limits,
    )?;
    let result = proto::Transaction::decode(bytes).map_err(decode_error)?;
    required_features(&result.required_features)?;
    Ok(result)
}

pub fn decode_semantic_command(
    bytes: &[u8],
    limits: &Limits,
) -> Result<proto::SemanticCommand, WireError> {
    preflight(bytes, Context::Command, limits.max_envelope_bytes, limits)?;
    proto::SemanticCommand::decode(bytes).map_err(decode_error)
}

fn decode_error(error: prost::DecodeError) -> WireError {
    WireError::Malformed(error.to_string())
}

#[derive(Clone, Copy)]
pub(crate) enum Context {
    Envelope,
    Snapshot,
    Transaction,
    Mutation,
    Record,
    Surface,
    Scene,
    Semantic,
    Text,
    Binding,
    Session,
    Ref,
    Rect,
    Point,
    Size,
    Affine,
    Color,
    Group,
    Solid,
    Path,
    Glyph,
    Radii,
    Rounded,
    Border,
    Stop,
    Linear,
    Radial,
    Stroke,
    Value,
    State,
    Relation,
    Action,
    Collection,
    Range,
    Cursor,
    Command,
    Preconditions,
    EmptyArguments,
    ValueArguments,
    TextArguments,
    SelectionArguments,
    ScrollArguments,
}

impl Context {
    fn rejects_variant(self, tag: u32) -> bool {
        // These schema ranges are reserved for required variant vocabulary.
        // Optional future metadata must use fields >=128 on these messages.
        match self {
            Self::Record => tag < 128 && !(1..=6).contains(&tag),
            Self::Mutation => tag < 128 && !(1..=2).contains(&tag),
            Self::Scene => tag < 128 && !matches!(tag,1..=6|10..=18|20),
            Self::Glyph => tag < 128 && !matches!(tag, 1..=7),
            Self::Radii | Self::Border => tag < 128 && !matches!(tag, 1..=4),
            Self::Rounded => tag < 128 && !matches!(tag, 1..=3),
            Self::Stop => tag < 128 && !matches!(tag, 1..=2),
            Self::Linear => tag < 128 && !matches!(tag, 1..=5),
            Self::Radial => tag < 128 && !matches!(tag, 1..=6),
            Self::Stroke => tag < 128 && !matches!(tag, 1..=8),
            _ => false,
        }
    }
    fn size(self) -> usize {
        use proto::*;
        macro_rules! sizes { ($($case:ident => $ty:ty),* $(,)?) => { match self { $(Self::$case => std::mem::size_of::<$ty>(),)* } }; }
        sizes! {
            Envelope=>Envelope, Snapshot=>Snapshot, Transaction=>Transaction, Mutation=>Mutation,
            Record=>Record, Surface=>Surface, Scene=>SceneNode, Semantic=>SemanticNode,
            Text=>TextModel, Binding=>Binding, Session=>SessionState, Ref=>ObjectRef,
            Rect=>Rect, Point=>Point, Size=>Size, Affine=>Affine, Color=>Color,
            Group=>Group, Solid=>SolidRect, Path=>FillPath, Glyph=>GlyphRun,
            Radii=>CornerRadii,Rounded=>RoundedRect,Border=>Border,Stop=>GradientStop,
            Linear=>LinearGradient,Radial=>RadialGradient,Stroke=>StrokePath,
            Value=>TypedValue, State=>StateValue, Relation=>Relation, Action=>Action,
            Collection=>CollectionModel, Range=>TextRange, Cursor=>Cursor,
            Command=>SemanticCommand, Preconditions=>Preconditions, EmptyArguments=>EmptyArguments,
            ValueArguments=>ValueArguments, TextArguments=>TextArguments,
            SelectionArguments=>SelectionArguments, ScrollArguments=>ScrollArguments,
        }
    }

    fn nested(self, tag: u32) -> Option<Self> {
        use Context::*;
        match (self, tag) {
            (Snapshot, 2) | (Mutation, 1) => Some(Record),
            (Transaction, 3) => Some(Mutation),
            (Record, 1) => Some(Surface),
            (Record, 2) => Some(Scene),
            (Record, 3) => Some(Semantic),
            (Record, 4) => Some(Text),
            (Record, 5) => Some(Binding),
            (Record, 6) => Some(Session),
            (Surface, 4 | 13 | 14 | 15)
            | (Scene, 3)
            | (Semantic, 8 | 11 | 13 | 14)
            | (Binding, 3..=5)
            | (Session, 5 | 6)
            | (Relation, 2)
            | (Collection, 2 | 5)
            | (Mutation, 2) => Some(Ref),
            (Surface, 5 | 8 | 16)
            | (Scene, 5)
            | (Solid | Rounded | Border | Linear | Radial, 1)
            | (Binding, 6)
            | (Session, 3) => Some(Rect),
            (Surface, 6) | (Radii, 1..=4) => Some(Size),
            (Surface, 7) | (Scene, 4) => Some(Affine),
            (Scene, 10) => Some(Group),
            (Scene, 11) => Some(Solid),
            (Scene, 12) => Some(Path),
            (Scene, 13) => Some(Glyph),
            (Scene, 14) => Some(Rounded),
            (Scene, 15) => Some(Border),
            (Scene, 16) => Some(Linear),
            (Scene, 17) => Some(Radial),
            (Scene, 18) => Some(Stroke),
            (Rounded | Border, 2) => Some(Radii),
            (Linear, 4) | (Radial, 5) => Some(Stop),
            (Glyph, 5) | (Cursor, 1 | 2) | (Linear, 2 | 3) | (Radial, 2) => Some(Point),
            (Solid | Path | Stop | Stroke, 2) | (Glyph, 7) | (Rounded, 3) | (Border, 4) => {
                Some(Color)
            }
            (Semantic, 6) => Some(Value),
            (Semantic, 7) => Some(State),
            (Semantic, 9) => Some(Relation),
            (Semantic, 10) => Some(Action),
            (Semantic, 12) => Some(Collection),
            (Text, 6) => Some(Range),
            (Session, 4) => Some(Cursor),
            (Command, 11) => Some(Preconditions),
            (Command, 12) => Some(EmptyArguments),
            (Command, 13) => Some(ValueArguments),
            (Command, 14) => Some(TextArguments),
            (Command, 15) => Some(SelectionArguments),
            (Command, 16) => Some(ScrollArguments),
            _ => None,
        }
    }
}

/// Scans lengths/counts without allocating generated messages. Charging each message,
/// field and byte is deliberately conservative, including vector/map allocation overhead.
pub(crate) fn preflight(
    bytes: &[u8],
    context: Context,
    max_bytes: usize,
    limits: &Limits,
) -> Result<usize, WireError> {
    if bytes.len() > max_bytes {
        return Err(WireError::Limit);
    }
    let mut budget = ScanBudget {
        allocation: bytes.len(),
        records: 0,
        fields: 0,
        items: 0,
    };
    scan(bytes, context, 0, &mut budget, limits)?;
    Ok(budget.allocation)
}

struct ScanBudget {
    allocation: usize,
    records: usize,
    fields: usize,
    items: usize,
}
fn charge(budget: &mut ScanBudget, n: usize, limits: &Limits) -> Result<(), WireError> {
    budget.allocation = budget.allocation.checked_add(n).ok_or(WireError::Limit)?;
    if budget.allocation > limits.max_memory_bytes {
        return Err(WireError::Limit);
    }
    Ok(())
}
fn scan(
    mut input: &[u8],
    context: Context,
    depth: usize,
    budget: &mut ScanBudget,
    limits: &Limits,
) -> Result<SceneValues, WireError> {
    if depth > 16 {
        return Err(WireError::Limit);
    }
    charge(budget, context.size() + 64, limits)?;
    let mut values = SceneValues {
        stops: usize::from(matches!(context, Context::Stop)),
        dashes: 0,
    };
    if matches!(context, Context::Record | Context::Mutation) {
        budget.records += 1;
        // Transaction counts both wrappers; snapshots count one wrapper per record.
        if budget.records > limits.max_records.saturating_mul(2) {
            return Err(WireError::Limit);
        }
    }
    while !input.is_empty() {
        budget.fields += 1;
        let fields_per_record = 128usize
            .saturating_add(limits.max_gradient_stops.saturating_mul(8))
            .saturating_add(limits.max_dash_segments);
        if budget.fields
            > limits
                .max_records
                .saturating_mul(fields_per_record)
                .max(128)
        {
            return Err(WireError::Limit);
        }
        charge(budget, 32, limits)?;
        let key = varint(&mut input)?;
        let tag = u32::try_from(key >> 3)
            .map_err(|_| WireError::Malformed("field number overflow".into()))?;
        if tag == 0 || tag > 0x1fff_ffff {
            return Err(WireError::Malformed("invalid field number".into()));
        }
        if context.rejects_variant(tag) {
            return Err(WireError::Unsupported(format!(
                "required variant field {tag}"
            )));
        }
        match key & 7 {
            0 => {
                varint(&mut input)?;
            }
            1 => {
                take(&mut input, 8)?;
                if matches!((context, tag), (Context::Stroke, 7)) {
                    values.dashes = values.dashes.saturating_add(1);
                }
            }
            2 => {
                let len = usize::try_from(varint(&mut input)?).map_err(|_| WireError::Limit)?;
                let data = take(&mut input, len)?;
                if matches!(
                    (context, tag),
                    (Context::Snapshot, 2) | (Context::Transaction, 3)
                ) {
                    budget.items += 1;
                    if budget.items > limits.max_records {
                        return Err(WireError::Limit);
                    }
                }
                if let Some(nested) = context.nested(tag) {
                    let child = scan(data, nested, depth + 1, budget, limits)?;
                    values.stops = values.stops.saturating_add(child.stops);
                    values.dashes = values.dashes.saturating_add(child.dashes);
                } else {
                    if matches!((context, tag), (Context::Stroke, 7)) {
                        if len % 8 != 0 {
                            return Err(WireError::Malformed("packed dash length".into()));
                        }
                        values.dashes = values.dashes.saturating_add(len / 8);
                    }
                    charge(budget, len.saturating_mul(8), limits)?;
                }
            }
            5 => {
                take(&mut input, 4)?;
            }
            _ => return Err(WireError::Malformed("unsupported wire type".into())),
        }
        if matches!(
            context,
            Context::Scene
                | Context::Record
                | Context::Mutation
                | Context::Linear
                | Context::Radial
                | Context::Stroke
        ) && (values.stops > limits.max_gradient_stops
            || values.dashes > limits.max_dash_segments)
        {
            return Err(WireError::Limit);
        }
    }
    Ok(values)
}

struct SceneValues {
    stops: usize,
    dashes: usize,
}

/// Scalar publication metadata inspection, after preflight, without allocation.
pub(crate) fn publication_header(
    mut input: &[u8],
    context: Context,
) -> Result<(u64, u64, usize), WireError> {
    let mut first = 0;
    let mut second = 0;
    let mut count = 0;
    while !input.is_empty() {
        let key = varint(&mut input)?;
        let tag = key >> 3;
        match key & 7 {
            0 => {
                let value = varint(&mut input)?;
                if tag == 1 {
                    first = value;
                }
                if tag == 2 {
                    second = value;
                }
            }
            1 => {
                take(&mut input, 8)?;
            }
            2 => {
                let len = usize::try_from(varint(&mut input)?).map_err(|_| WireError::Limit)?;
                take(&mut input, len)?;
                if matches!(
                    (context, tag),
                    (Context::Snapshot, 2) | (Context::Transaction, 3)
                ) {
                    count += 1;
                }
            }
            5 => {
                take(&mut input, 4)?;
            }
            _ => return Err(WireError::Malformed("unsupported wire type".into())),
        }
    }
    Ok((first, second, count))
}
fn take<'a>(input: &mut &'a [u8], len: usize) -> Result<&'a [u8], WireError> {
    if len > input.len() {
        return Err(WireError::Malformed("truncated field".into()));
    }
    let (head, tail) = input.split_at(len);
    *input = tail;
    Ok(head)
}
fn varint(input: &mut &[u8]) -> Result<u64, WireError> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *take(input, 1)?.first().unwrap();
        if shift == 63 && byte > 1 {
            return Err(WireError::Malformed("varint overflow".into()));
        }
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(WireError::Malformed("varint overflow".into()))
}
