//! Safe retained state and atomic, full-record reduction of exact Protobuf bytes.
use crate::publication::{AssembledPublication, PublicationKind};
use crate::resources::{MemoryBudget, MemoryError, MemoryReservation};
use crate::wire::{self, proto as pb};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub type ResourceId = [u8; 32];

fn set_memory(len: usize) -> usize {
    len.saturating_mul(96)
        .saturating_add(if len == 0 { 0 } else { 512 })
}
fn record_map_memory(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    // Standard BTree nodes have eleven entry slots; non-root nodes retain at
    // least five entries. Include root slack and internal child pointers.
    let node_bytes = 11usize
        .saturating_mul(std::mem::size_of::<Record>() + 8)
        .saturating_add(128);
    (len / 5 + 1).saturating_mul(node_bytes)
}
fn vector_memory<T>(value: &Vec<T>) -> usize {
    value
        .capacity()
        .saturating_mul(std::mem::size_of::<T>())
        .saturating_add(if value.capacity() == 0 { 0 } else { 32 })
}
fn string_memory(value: &String) -> usize {
    value
        .capacity()
        .saturating_add(if value.capacity() == 0 { 0 } else { 32 })
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_envelope_bytes: usize,
    pub max_transaction_bytes: usize,
    pub max_snapshot_bytes: usize,
    pub max_memory_bytes: usize,
    pub max_records: usize,
    pub max_depth: usize,
    pub max_retired: usize,
    pub max_history: usize,
    pub max_string_bytes: usize,
    pub max_geometry: f64,
    pub max_gradient_stops: usize,
    pub max_dash_segments: usize,
    pub max_stroke_miter_limit: f64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_envelope_bytes: 1 << 20,
            max_transaction_bytes: 16 << 20,
            max_snapshot_bytes: 64 << 20,
            max_memory_bytes: 128 << 20,
            max_records: 100_000,
            max_depth: 128,
            max_retired: 100_000,
            max_history: 256,
            max_string_bytes: 1 << 20,
            max_geometry: 1e9,
            max_gradient_stops: 4096,
            max_dash_segments: 1024,
            max_stroke_miter_limit: 32.0,
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ModelError {
    #[error(transparent)]
    Wire(#[from] wire::WireError),
    #[error("invalid record: {0}")]
    Invalid(String),
    #[error("receiver limit exceeded: {0}")]
    Limit(&'static str),
    #[error("unresolved or mistyped object reference: {0}")]
    Reference(u64),
    #[error("invalid object version for {0}")]
    Version(u64),
    #[error("deleted ID is retired: {0}")]
    Retired(u64),
    #[error("duplicate object or mutation ID: {0}")]
    DuplicateId(u64),
    #[error("revision mismatch; reset or replay from committed revision {current}")]
    Revision { current: u64 },
    #[error("revision was retransmitted with conflicting exact bytes")]
    DigestConflict,
    #[error("ownership cycle, duplicate parent, or orphan: {0}")]
    Ownership(u64),
    #[error("unsupported required mutation or primitive")]
    Unsupported,
    #[error("presentation does not refer to a known applied revision")]
    Presentation,
    #[error("a validated publication is waiting for resources")]
    Pending,
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error("publication metadata differs from decoded payload")]
    PublicationMetadata,
}

macro_rules! vocabulary {
    ($name:ident { $($variant:ident = $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
        pub enum $name { $($variant),+ }
        impl $name {
            fn from_wire(value:i32)->Result<Self,ModelError> {
                match value { $($value=>Ok(Self::$variant),)+ _=>Err(ModelError::Unsupported) }
            }
        }
    };
}
vocabulary!(ObjectKind { Surface=1, Scene=2, Semantic=3, Text=4, Binding=5, Session=6 });
vocabulary!(SurfaceRole { Toplevel=1, Popup=2, Tooltip=3, Shell=4 });
vocabulary!(VisualCoverage { Complete=1, Partial=2, Unsupported=3 });
vocabulary!(SemanticCoverage { Rich=1, Partial=2, Opaque=3 });
vocabulary!(CaptureConsistency { Coherent=1, Observed=2 });
vocabulary!(FillRule { Nonzero=1, EvenOdd=2 });
vocabulary!(GradientSpread { Pad=1 });
vocabulary!(StrokeCap { Butt=1, Round=2, Square=3 });
vocabulary!(StrokeJoin { Miter=1, Round=2, Bevel=3 });
vocabulary!(SemanticRole { Root=1, Group=2, Button=3, Checkbox=4, TextField=5, Label=6, List=7, ListItem=8, CustomCanvas=9 });
vocabulary!(StateKey { Disabled=1, Checked=2, Selected=3, Expanded=4, ReadOnly=5 });
vocabulary!(TriState { False=1, True=2, Unknown=3 });
vocabulary!(RelationKind { LabelFor=1, LabelledBy=2, Controls=3, Describes=4, MemberOf=5 });
vocabulary!(Sensitivity { Public=1, Sensitive=2, Redacted=3 });
vocabulary!(Completeness { Full=1, Incomplete=2, Opaque=3 });
vocabulary!(Materialization { Full=1, Partial=2 });
vocabulary!(TextStatus { Available=1, Unavailable=2, Redacted=3 });
vocabulary!(CursorShape { Default=1, Text=2, Pointer=3, Hidden=4 });

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ObjectRef {
    pub kind: ObjectKind,
    pub id: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Affine {
    pub xx: f64,
    pub yx: f64,
    pub xy: f64,
    pub yy: f64,
    pub tx: f64,
    pub ty: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Color {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CornerRadii {
    pub top_left: Size,
    pub top_right: Size,
    pub bottom_right: Size,
    pub bottom_left: Size,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct GradientStop {
    pub offset: f64,
    pub color: Color,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Surface {
    pub id: u64,
    pub version: u64,
    pub role: SurfaceRole,
    pub parent: Option<ObjectRef>,
    pub desktop_rect: Rect,
    pub content_size: Size,
    pub transform: Affine,
    pub clip: Option<Rect>,
    pub opacity: f64,
    pub stacking_order: i64,
    pub mapped: bool,
    pub focused: bool,
    pub modal_for: Option<ObjectRef>,
    pub scene_root: Option<ObjectRef>,
    pub semantic_root: Option<ObjectRef>,
    pub input_region: Vec<Rect>,
    pub routing_token: Vec<u8>,
    pub visual_coverage: VisualCoverage,
    pub semantic_coverage: SemanticCoverage,
    pub capture_consistency: CaptureConsistency,
    pub source_capture_tokens: Vec<Vec<u8>>,
    pub unsupported_features: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum GlyphFont {
    Fingerprint(ResourceId),
    Outline(ResourceId),
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Primitive {
    Group,
    SolidRect {
        rect: Rect,
        color: Color,
    },
    FillPath {
        resource: ResourceId,
        color: Color,
        fill_rule: FillRule,
    },
    GlyphRun {
        font: GlyphFont,
        size: f64,
        glyph_ids: Vec<u32>,
        positions: Vec<Point>,
        scalar_clusters: Vec<u64>,
        color: Color,
    },
    RoundedRect {
        rect: Rect,
        radii: CornerRadii,
        color: Color,
    },
    Border {
        rect: Rect,
        radii: CornerRadii,
        width: f64,
        color: Color,
    },
    LinearGradient {
        rect: Rect,
        start: Point,
        end: Point,
        stops: Vec<GradientStop>,
        spread: GradientSpread,
    },
    RadialGradient {
        rect: Rect,
        center: Point,
        inner_radius: f64,
        outer_radius: f64,
        stops: Vec<GradientStop>,
        spread: GradientSpread,
    },
    StrokePath {
        resource: ResourceId,
        color: Color,
        width: f64,
        cap: StrokeCap,
        join: StrokeJoin,
        miter_limit: f64,
        dash_lengths: Vec<f64>,
        dash_offset: f64,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SceneNode {
    pub id: u64,
    pub version: u64,
    pub children: Vec<ObjectRef>,
    pub transform: Affine,
    pub clip: Option<Rect>,
    pub opacity: f64,
    pub primitive: Primitive,
    pub resources: BTreeSet<ResourceId>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum TypedValue {
    Text(String),
    Boolean(bool),
    Number(f64),
    Integer(i64),
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Relation {
    pub kind: RelationKind,
    pub targets: Vec<ObjectRef>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Action {
    pub id: String,
    pub schema_version: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CollectionModel {
    pub version: u64,
    pub items: Vec<ObjectRef>,
    pub total_count: Option<u64>,
    pub materialization: Materialization,
    pub selected_items: Vec<ObjectRef>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SemanticNode {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser_input_type: Option<String>,
    pub id: u64,
    pub version: u64,
    pub role: SemanticRole,
    pub label: Option<String>,
    pub description: Option<String>,
    pub value: Option<TypedValue>,
    pub states: BTreeMap<StateKey, TriState>,
    pub children: Vec<ObjectRef>,
    pub relations: Vec<Relation>,
    pub actions: Vec<Action>,
    pub text: Option<ObjectRef>,
    pub collection: Option<CollectionModel>,
    pub bindings: Vec<ObjectRef>,
    pub semantic_group: Option<ObjectRef>,
    pub reading_order: Option<u64>,
    pub sensitivity: Sensitivity,
    pub completeness: Completeness,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TextRange {
    pub anchor: u64,
    pub focus: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextModel {
    pub id: u64,
    pub version: u64,
    pub text_version: u64,
    pub status: TextStatus,
    pub content: Option<String>,
    pub selection: Option<TextRange>,
    pub editable: bool,
    pub selectable: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Binding {
    pub id: u64,
    pub version: u64,
    pub surface: ObjectRef,
    pub semantics: Vec<ObjectRef>,
    pub scenes: Vec<ObjectRef>,
    pub interaction_region: Option<Rect>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum CursorAppearance {
    Shape(CursorShape),
    Outline(ResourceId),
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cursor {
    pub position: Point,
    pub hotspot: Point,
    pub appearance: CursorAppearance,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionState {
    pub id: u64,
    pub version: u64,
    pub canvas: Rect,
    pub cursor: Option<Cursor>,
    pub focus: Option<ObjectRef>,
    pub focus_surface: Option<ObjectRef>,
    pub focus_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Record {
    Surface(Surface),
    Scene(SceneNode),
    Semantic(SemanticNode),
    Text(TextModel),
    Binding(Binding),
    Session(SessionState),
}
impl Record {
    pub fn id(&self) -> u64 {
        match self {
            Self::Surface(v) => v.id,
            Self::Scene(v) => v.id,
            Self::Semantic(v) => v.id,
            Self::Text(v) => v.id,
            Self::Binding(v) => v.id,
            Self::Session(v) => v.id,
        }
    }
    pub fn version(&self) -> u64 {
        match self {
            Self::Surface(v) => v.version,
            Self::Scene(v) => v.version,
            Self::Semantic(v) => v.version,
            Self::Text(v) => v.version,
            Self::Binding(v) => v.version,
            Self::Session(v) => v.version,
        }
    }
    pub fn kind(&self) -> ObjectKind {
        match self {
            Self::Surface(_) => ObjectKind::Surface,
            Self::Scene(_) => ObjectKind::Scene,
            Self::Semantic(_) => ObjectKind::Semantic,
            Self::Text(_) => ObjectKind::Text,
            Self::Binding(_) => ObjectKind::Binding,
            Self::Session(_) => ObjectKind::Session,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct State {
    pub revision: u64,
    pub records: BTreeMap<u64, Record>,
    pub retired: BTreeSet<u64>,
}
impl State {
    /// Convert a trusted, already bounded and allocated cooperating-source
    /// capture directly into validated model records, without wire decoding.
    /// The caller must hold a conservative allocation reservation throughout
    /// conversion and retention. This does not establish source provenance or
    /// authorize/check resource availability.
    pub fn from_source_records(
        revision: u64,
        records: Vec<pb::Record>,
        limits: &Limits,
    ) -> Result<Self, ModelError> {
        if revision == 0 {
            return Err(invalid("zero source revision"));
        }
        if records.len() > limits.max_records {
            return Err(ModelError::Limit("records"));
        }
        let mut state = Self {
            revision,
            records: BTreeMap::new(),
            retired: BTreeSet::new(),
        };
        for raw in records {
            let record = convert_record(raw, limits)?;
            let id = record.id();
            if state.records.insert(id, record).is_some() {
                return Err(ModelError::DuplicateId(id));
            }
        }
        if state.estimated_bytes() > limits.max_memory_bytes {
            return Err(ModelError::Limit("decoded source state"));
        }
        validate_graph(&state, limits)?;
        Ok(state)
    }
    pub fn resource_refs(&self) -> BTreeSet<ResourceId> {
        let mut result = BTreeSet::new();
        for record in self.records.values() {
            match record {
                Record::Scene(scene) => {
                    result.extend(&scene.resources);
                    match &scene.primitive {
                        Primitive::FillPath { resource, .. }
                        | Primitive::StrokePath { resource, .. } => {
                            result.insert(*resource);
                        }
                        Primitive::GlyphRun { font, .. } => {
                            result.insert(match font {
                                GlyphFont::Fingerprint(v) | GlyphFont::Outline(v) => *v,
                            });
                        }
                        _ => {}
                    }
                }
                Record::Session(SessionState {
                    cursor:
                        Some(Cursor {
                            appearance: CursorAppearance::Outline(resource),
                            ..
                        }),
                    ..
                }) => {
                    result.insert(*resource);
                }
                _ => {}
            }
        }
        result
    }
    pub fn estimated_bytes(&self) -> usize {
        let mut bytes = set_memory(self.retired.len())
            .saturating_add(record_map_memory(self.records.len()))
            .saturating_add(std::mem::size_of::<Self>());
        for record in self.records.values() {
            // Allocation headers and fixed record metadata; heap capacities below
            // can exceed logical lengths after Protobuf merges singular fields.
            bytes = bytes.saturating_add(256);
            let heap = match record {
                Record::Surface(s) => {
                    vector_memory(&s.input_region)
                        + vector_memory(&s.routing_token)
                        + vector_memory(&s.source_capture_tokens)
                        + s.source_capture_tokens
                            .iter()
                            .map(vector_memory)
                            .sum::<usize>()
                        + vector_memory(&s.unsupported_features)
                        + s.unsupported_features
                            .iter()
                            .map(string_memory)
                            .sum::<usize>()
                }
                Record::Scene(s) => {
                    vector_memory(&s.children)
                        + set_memory(s.resources.len())
                        + match &s.primitive {
                            Primitive::GlyphRun {
                                glyph_ids,
                                positions,
                                scalar_clusters,
                                ..
                            } => {
                                vector_memory(glyph_ids)
                                    + vector_memory(positions)
                                    + vector_memory(scalar_clusters)
                            }
                            Primitive::LinearGradient { stops, .. }
                            | Primitive::RadialGradient { stops, .. } => vector_memory(stops),
                            Primitive::StrokePath { dash_lengths, .. } => {
                                vector_memory(dash_lengths)
                            }
                            _ => 0,
                        }
                }
                Record::Semantic(s) => {
                    s.label.as_ref().map_or(0, string_memory)
                        + s.description.as_ref().map_or(0, string_memory)
                        + s.browser_input_type.as_ref().map_or(0, string_memory)
                        + match &s.value {
                            Some(TypedValue::Text(v)) => string_memory(v),
                            _ => 0,
                        }
                        + s.states.len() * 64
                        + if s.states.is_empty() { 0 } else { 256 }
                        + vector_memory(&s.children)
                        + vector_memory(&s.bindings)
                        + vector_memory(&s.relations)
                        + s.relations
                            .iter()
                            .map(|r| vector_memory(&r.targets))
                            .sum::<usize>()
                        + vector_memory(&s.actions)
                        + s.actions
                            .iter()
                            .map(|a| string_memory(&a.id))
                            .sum::<usize>()
                        + s.collection.as_ref().map_or(0, |c| {
                            128 + vector_memory(&c.items) + vector_memory(&c.selected_items)
                        })
                }
                Record::Text(t) => t.content.as_ref().map_or(0, string_memory),
                Record::Binding(b) => vector_memory(&b.semantics) + vector_memory(&b.scenes),
                Record::Session(_) => 0,
            };
            bytes = bytes.saturating_add(heap);
        }
        bytes
    }
}

/// An owned resource request keeps its allocation charged until dropped.
/// It deliberately cannot be cloned; retaining requests consumes receiver memory.
#[derive(Debug)]
pub struct MissingResources {
    ids: BTreeSet<ResourceId>,
    _memory: MemoryReservation,
}
impl std::ops::Deref for MissingResources {
    type Target = BTreeSet<ResourceId>;
    fn deref(&self) -> &Self::Target {
        &self.ids
    }
}
impl PartialEq for MissingResources {
    fn eq(&self, other: &Self) -> bool {
        self.ids == other.ids
    }
}
impl Eq for MissingResources {}
impl PartialEq<BTreeSet<ResourceId>> for MissingResources {
    fn eq(&self, other: &BTreeSet<ResourceId>) -> bool {
        &self.ids == other
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied {
        revision: u64,
    },
    Duplicate {
        revision: u64,
    },
    PendingResources {
        revision: u64,
        missing: MissingResources,
    },
}

struct PendingState {
    declared_only_graph_resources: bool,
    state: State,
    resources: BTreeSet<ResourceId>,
    digest: ResourceId,
    memory: MemoryReservation,
}
pub struct Replica {
    state: State,
    limits: Limits,
    history: BTreeMap<u64, ResourceId>,
    presented_revision: Option<u64>,
    budget: MemoryBudget,
    state_memory: MemoryReservation,
    history_memory: MemoryReservation,
    live_resources: BTreeSet<ResourceId>,
    pending: Option<PendingState>,
}
impl Replica {
    pub fn new(limits: Limits) -> Self {
        let budget = MemoryBudget::new(limits.max_memory_bytes);
        Self::with_memory_budget(limits, budget)
    }
    pub fn with_memory_budget(limits: Limits, budget: MemoryBudget) -> Self {
        let state_memory = budget.reserve(0).expect("zero reservation");
        let history_memory = budget.reserve(0).expect("zero reservation");
        Self {
            state: State::default(),
            limits,
            history: BTreeMap::new(),
            presented_revision: None,
            budget,
            state_memory,
            history_memory,
            live_resources: BTreeSet::new(),
            pending: None,
        }
    }
    /// Independent bounded replay of this exact committed predecessor. Used by
    /// trusted producer validation; pending or unverified state is never forked.
    pub fn fork(&self, limits: Limits) -> Result<Self, ModelError> {
        if self.pending.is_some() {
            return Err(ModelError::Pending);
        }
        if limits.max_memory_bytes > self.limits.max_memory_bytes
            || self.history.len() > limits.max_history
            || self.state.records.len() > limits.max_records
            || self.state.retired.len() > limits.max_retired
            || self
                .state_memory
                .bytes()
                .saturating_add(self.history_memory.bytes())
                > limits.max_memory_bytes
        {
            return Err(ModelError::Limit("fork limits"));
        }
        validate_graph(&self.state, &limits)?;
        // All copies share the original ledger: replay cannot multiply the
        // caller's memory ceiling by creating independent accounting pools.
        let mut fork = Self::with_memory_budget(limits, self.budget.clone());
        fork.state_memory.resize(self.state_memory.bytes())?;
        fork.history_memory.resize(self.history_memory.bytes())?;
        fork.state = self.state.clone();
        fork.live_resources = self.live_resources.clone();
        fork.history = self.history.clone();
        fork.presented_revision = self.presented_revision;
        Ok(fork)
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    pub fn resource_refs(&self) -> &BTreeSet<ResourceId> {
        &self.live_resources
    }
    /// True when every explicit staging dependency is referenced by the parsed
    /// graph. Trusted producer adapters may require this stricter grant policy;
    /// the reducer itself continues accepting separate explicit dependencies.
    pub fn pending_declares_only_graph_resources(&self) -> Option<bool> {
        self.pending
            .as_ref()
            .map(|p| p.declared_only_graph_resources)
    }
    pub fn pending_resources(&self) -> Option<&BTreeSet<ResourceId>> {
        self.pending.as_ref().map(|v| &v.resources)
    }
    pub(crate) fn pending_state(&self) -> Option<&State> {
        self.pending.as_ref().map(|v| &v.state)
    }
    pub(crate) fn reset_presentation(&mut self) {
        self.presented_revision = None;
    }
    pub fn abort_pending(&mut self) -> bool {
        self.pending.take().is_some()
    }
    pub fn resume_pending(
        &mut self,
        available: &BTreeSet<ResourceId>,
    ) -> Result<ApplyOutcome, ModelError> {
        let pending = self.pending.as_ref().ok_or(ModelError::Pending)?;
        if pending.resources.difference(available).next().is_some() {
            return Ok(ApplyOutcome::PendingResources {
                revision: pending.state.revision,
                missing: self.missing_output(&pending.resources, available)?,
            });
        }
        // Growing history is the last fallible commit step. Keep the validated
        // pending graph owned and retryable if another receiver fills the ledger.
        self.reserve_history()?;
        let pending = self.pending.take().unwrap();
        Ok(self.install_reserved(pending))
    }
    /// Compares outer publication declarations with actual payload before staging.
    pub fn apply_publication(
        &mut self,
        publication: &AssembledPublication,
        available: &BTreeSet<ResourceId>,
    ) -> Result<ApplyOutcome, ModelError> {
        let begin = publication.begin();
        let bytes = publication.bytes();
        let context = match begin.kind {
            PublicationKind::Snapshot { .. } => wire::Context::Snapshot,
            PublicationKind::Transaction { .. } => wire::Context::Transaction,
        };
        wire::preflight(bytes, context, self.limits.max_snapshot_bytes, &self.limits)?;
        let (first, second, count) = wire::publication_header(bytes, context)?;
        let matches = match begin.kind {
            PublicationKind::Snapshot { revision } => {
                first == revision && count == begin.item_count as usize
            }
            PublicationKind::Transaction {
                base_revision,
                new_revision,
            } => {
                first == base_revision
                    && second == new_revision
                    && count == begin.item_count as usize
            }
        };
        if !matches {
            return Err(ModelError::PublicationMetadata);
        }
        match begin.kind {
            PublicationKind::Snapshot { .. } => self.apply_snapshot(bytes, available),
            PublicationKind::Transaction { .. } => self.apply_transaction(bytes, available),
        }
    }
    pub fn presented_revision(&self) -> Option<u64> {
        self.presented_revision
    }
    pub fn mark_presented(&mut self, revision: u64) -> Result<(), ModelError> {
        if revision == 0
            || revision > self.state.revision
            || (revision != self.state.revision && !self.history.contains_key(&revision))
            || self.presented_revision.is_some_and(|old| revision < old)
        {
            return Err(ModelError::Presentation);
        }
        self.presented_revision = Some(revision);
        Ok(())
    }
    fn known(
        &self,
        revision: u64,
        digest: &ResourceId,
        available: &BTreeSet<ResourceId>,
    ) -> Result<Option<ApplyOutcome>, ModelError> {
        if let Some(pending) = &self.pending {
            if pending.state.revision != revision {
                return Err(ModelError::Pending);
            }
            if &pending.digest != digest {
                return Err(ModelError::DigestConflict);
            }
            return Ok(Some(ApplyOutcome::PendingResources {
                revision,
                missing: self.missing_output(&pending.resources, available)?,
            }));
        }
        if let Some(known) = self.history.get(&revision) {
            if known != digest {
                return Err(ModelError::DigestConflict);
            }
            return Ok(Some(ApplyOutcome::Duplicate { revision }));
        }
        Ok(None)
    }
    pub fn apply_snapshot(
        &mut self,
        bytes: &[u8],
        available: &BTreeSet<ResourceId>,
    ) -> Result<ApplyOutcome, ModelError> {
        let allocation = wire::preflight(
            bytes,
            wire::Context::Snapshot,
            self.limits.max_snapshot_bytes,
            &self.limits,
        )?;
        let revision = wire::publication_header(bytes, wire::Context::Snapshot)?.0;
        let digest: ResourceId = Sha256::digest(bytes).into();
        if let Some(outcome) = self.known(revision, &digest, available)? {
            return Ok(outcome);
        }
        let memory = self.reserve_stage(allocation)?;
        let snapshot = wire::decode_snapshot(bytes, &self.limits)?;
        drop(snapshot.required_features);
        if snapshot.revision == 0 || snapshot.revision <= self.state.revision {
            return Err(ModelError::Revision {
                current: self.state.revision,
            });
        }
        if snapshot.records.len() > self.limits.max_records {
            return Err(ModelError::Limit("records"));
        }
        let mut candidate = State {
            revision: snapshot.revision,
            records: BTreeMap::new(),
            retired: self.state.retired.clone(),
        };
        for wire_record in snapshot.records {
            let record = convert_record(wire_record, &self.limits)?;
            let id = record.id();
            if candidate.retired.contains(&id) {
                return Err(ModelError::Retired(id));
            }
            if let Some(old) = self.state.records.get(&id) {
                if old.kind() != record.kind()
                    || record.version() < old.version()
                    || (record.version() == old.version() && &record != old)
                {
                    return Err(ModelError::Version(id));
                }
                validate_snapshot_versions(old, &record)?;
            }
            if candidate.records.insert(id, record).is_some() {
                return Err(ModelError::DuplicateId(id));
            }
        }
        for id in self.state.records.keys() {
            if !candidate.records.contains_key(id) {
                candidate.retired.insert(*id);
            }
        }
        self.commit(
            &mut candidate,
            false,
            snapshot.required_resources,
            digest,
            memory,
            available,
        )
    }
    pub fn apply_transaction(
        &mut self,
        bytes: &[u8],
        available: &BTreeSet<ResourceId>,
    ) -> Result<ApplyOutcome, ModelError> {
        let allocation = wire::preflight(
            bytes,
            wire::Context::Transaction,
            self.limits.max_transaction_bytes,
            &self.limits,
        )?;
        let revision = wire::publication_header(bytes, wire::Context::Transaction)?.1;
        let digest: ResourceId = Sha256::digest(bytes).into();
        if let Some(outcome) = self.known(revision, &digest, available)? {
            return Ok(outcome);
        }
        let memory = self.reserve_stage(allocation)?;
        let transaction = wire::decode_transaction(bytes, &self.limits)?;
        drop(transaction.required_features);
        if self.state.revision == 0
            || transaction.base_revision != self.state.revision
            || transaction.base_revision.checked_add(1) != Some(transaction.new_revision)
        {
            return Err(ModelError::Revision {
                current: self.state.revision,
            });
        }
        if transaction.mutations.len() > self.limits.max_records {
            return Err(ModelError::Limit("mutations"));
        }
        let original_memory = std::mem::replace(&mut self.state_memory, self.budget.reserve(0)?);
        let mut candidate = std::mem::take(&mut self.state);
        let prior_revision = candidate.revision;
        candidate.revision = transaction.new_revision;
        let mut undo = Vec::new();
        let result = (|| {
            let mut seen = BTreeSet::new();
            for mutation in transaction.mutations {
                match mutation.mutation.ok_or(ModelError::Unsupported)? {
                    pb::mutation::Mutation::Put(wire_record) => {
                        let record = convert_record(wire_record, &self.limits)?;
                        let id = record.id();
                        if !seen.insert(id) {
                            return Err(ModelError::DuplicateId(id));
                        }
                        if candidate.retired.contains(&id) {
                            return Err(ModelError::Retired(id));
                        }
                        match candidate.records.get(&id) {
                            Some(old)
                                if old.kind() != record.kind()
                                    || old.version().checked_add(1) != Some(record.version()) =>
                            {
                                return Err(ModelError::Version(id))
                            }
                            None if record.version() != 1 => return Err(ModelError::Version(id)),
                            _ => {}
                        }
                        if let Some(old) = candidate.records.get(&id) {
                            validate_content_versions(old, &record)?;
                        }
                        undo.push((id, candidate.records.insert(id, record)));
                    }
                    pb::mutation::Mutation::Delete(wire_ref) => {
                        let reference = convert_ref(wire_ref)?;
                        if !seen.insert(reference.id) {
                            return Err(ModelError::DuplicateId(reference.id));
                        }
                        let old = candidate
                            .records
                            .get(&reference.id)
                            .ok_or(ModelError::Reference(reference.id))?;
                        if old.kind() != reference.kind {
                            return Err(ModelError::Reference(reference.id));
                        }
                        undo.push((reference.id, candidate.records.remove(&reference.id)));
                        candidate.retired.insert(reference.id);
                    }
                }
            }
            self.commit(
                &mut candidate,
                true,
                transaction.required_resources,
                digest,
                memory,
                available,
            )
        })();
        if !matches!(result, Ok(ApplyOutcome::Applied { .. })) {
            for (id, old) in undo {
                match old {
                    Some(record) => {
                        candidate.records.insert(id, record);
                    }
                    None => {
                        candidate.records.remove(&id);
                    }
                }
                candidate.retired.remove(&id);
            }
            candidate.revision = prior_revision;
            self.state = candidate;
            self.state_memory = original_memory;
        } else {
            drop(undo);
            drop(original_memory);
        }
        result
    }
    fn reserve_stage(&self, allocation: usize) -> Result<MemoryReservation, ModelError> {
        let bytes = allocation
            .checked_mul(3)
            .and_then(|v| v.checked_add(self.state.estimated_bytes().saturating_mul(2)))
            .and_then(|v| v.checked_add(record_map_memory(1).saturating_add(4096)))
            .ok_or(ModelError::Limit("allocation overflow"))?;
        if bytes
            .saturating_add(self.state_memory.bytes())
            .saturating_add(self.history_memory.bytes())
            .saturating_add(self.pending.as_ref().map_or(0, |p| p.memory.bytes()))
            > self.limits.max_memory_bytes
        {
            return Err(ModelError::Limit("decoded staging"));
        }
        Ok(self.budget.reserve(bytes)?)
    }
    fn commit(
        &mut self,
        candidate: &mut State,
        preserve_pending_predecessor: bool,
        required: Vec<Vec<u8>>,
        digest: ResourceId,
        mut memory: MemoryReservation,
        available: &BTreeSet<ResourceId>,
    ) -> Result<ApplyOutcome, ModelError> {
        if candidate.records.len() > self.limits.max_records {
            return Err(ModelError::Limit("records"));
        }
        if candidate.retired.len() > self.limits.max_retired {
            return Err(ModelError::Limit("retired IDs; start a new epoch"));
        }
        validate_graph(candidate, &self.limits)?;
        let mut resources = candidate.resource_refs();
        let mut declared_only_graph_resources = true;
        for resource in required {
            let resource = resource_id(resource)?;
            declared_only_graph_resources &= resources.contains(&resource);
            resources.insert(resource);
        }
        let has_missing = resources.difference(available).next().is_some();
        memory.resize(
            candidate
                .estimated_bytes()
                .saturating_add(set_memory(resources.len())),
        )?;
        let missing = if has_missing {
            Some(self.missing_output(&resources, available)?)
        } else {
            self.reserve_history()?;
            None
        };
        let pending = PendingState {
            declared_only_graph_resources,
            state: if has_missing && preserve_pending_predecessor {
                candidate.clone()
            } else {
                std::mem::take(candidate)
            },
            resources,
            digest,
            memory,
        };
        if let Some(missing) = missing {
            let revision = pending.state.revision;
            self.pending = Some(pending);
            return Ok(ApplyOutcome::PendingResources { revision, missing });
        }
        Ok(self.install_reserved(pending))
    }
    fn reserve_history(&mut self) -> Result<(), ModelError> {
        let history_count = (self.history.len() + 1).min(self.limits.max_history);
        self.history_memory.resize(
            history_count
                .saturating_mul(128)
                .saturating_add(if history_count == 0 { 0 } else { 640 }),
        )?;
        Ok(())
    }
    fn missing_output(
        &self,
        resources: &BTreeSet<ResourceId>,
        available: &BTreeSet<ResourceId>,
    ) -> Result<MissingResources, ModelError> {
        let count = resources.difference(available).count();
        let memory = self.budget.reserve(set_memory(count))?;
        let ids = resources.difference(available).copied().collect();
        Ok(MissingResources {
            ids,
            _memory: memory,
        })
    }
    fn install_reserved(&mut self, pending: PendingState) -> ApplyOutcome {
        let revision = pending.state.revision;
        // No fallible operations follow: records, resources and revision swap together.
        self.state = pending.state;
        self.live_resources = pending.resources;
        self.state_memory = pending.memory;
        if self.limits.max_history > 0 {
            if self.history.len() == self.limits.max_history {
                let oldest = *self.history.keys().next().unwrap();
                self.history.remove(&oldest);
            }
            self.history.insert(revision, pending.digest);
        }
        ApplyOutcome::Applied { revision }
    }
}

// Initial content clocks: unchanged text (including selection-only replacement)
// retains its clock; content/status changes advance exactly one. Collection clocks
// likewise advance only when materialization, membership, count or selection changes.
fn validate_content_versions(old: &Record, new: &Record) -> Result<(), ModelError> {
    match (old, new) {
        (Record::Text(old), Record::Text(new)) => {
            let changed = old.content != new.content || old.status != new.status;
            let expected = if changed {
                old.text_version.checked_add(1)
            } else {
                Some(old.text_version)
            };
            if expected != Some(new.text_version) {
                return Err(ModelError::Version(new.id));
            }
        }
        (Record::Semantic(old), Record::Semantic(new)) => {
            match (&old.collection, &new.collection) {
                (Some(old_collection), Some(new_collection)) => {
                    let changed = collection_changed(old_collection, new_collection);
                    let expected = if changed {
                        old_collection.version.checked_add(1)
                    } else {
                        Some(old_collection.version)
                    };
                    if expected != Some(new_collection.version) {
                        return Err(ModelError::Version(new.id));
                    }
                }
                (None, Some(collection)) if collection.version != 1 => {
                    return Err(ModelError::Version(new.id))
                }
                _ => {}
            }
        }
        (Record::Session(old), Record::Session(new)) => {
            let changed = old.focus != new.focus || old.focus_surface != new.focus_surface;
            let expected = if changed {
                old.focus_generation.checked_add(1)
            } else {
                Some(old.focus_generation)
            };
            if expected != Some(new.focus_generation) {
                return Err(ModelError::Version(new.id));
            }
        }
        _ => {}
    }
    Ok(())
}

fn collection_changed(old: &CollectionModel, new: &CollectionModel) -> bool {
    old.items != new.items
        || old.total_count != new.total_count
        || old.materialization != new.materialization
        || old.selected_items != new.selected_items
}
fn validate_snapshot_versions(old: &Record, new: &Record) -> Result<(), ModelError> {
    let valid = match (old, new) {
        (Record::Text(old), Record::Text(new)) => {
            new.text_version >= old.text_version
                && ((old.content == new.content && old.status == new.status)
                    || new.text_version > old.text_version)
        }
        (Record::Semantic(old), Record::Semantic(new)) => {
            match (&old.collection, &new.collection) {
                (Some(old), Some(new)) => {
                    new.version >= old.version
                        && (!collection_changed(old, new) || new.version > old.version)
                }
                _ => true,
            }
        }
        (Record::Session(old), Record::Session(new)) => {
            new.focus_generation >= old.focus_generation
                && ((old.focus == new.focus && old.focus_surface == new.focus_surface)
                    || new.focus_generation > old.focus_generation)
        }
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(ModelError::Version(new.id()))
    }
}

fn invalid(message: &str) -> ModelError {
    ModelError::Invalid(message.into())
}
fn identity(id: u64, version: u64) -> Result<(), ModelError> {
    if id == 0 || version == 0 {
        Err(invalid("zero ID/version"))
    } else {
        Ok(())
    }
}
fn convert_ref(reference: pb::ObjectRef) -> Result<ObjectRef, ModelError> {
    if reference.id == 0 {
        return Err(invalid("zero reference"));
    }
    Ok(ObjectRef {
        kind: ObjectKind::from_wire(reference.kind)?,
        id: reference.id,
    })
}
fn optional_ref(
    value: Option<pb::ObjectRef>,
    kind: ObjectKind,
) -> Result<Option<ObjectRef>, ModelError> {
    value.map(|v| typed_ref(v, kind)).transpose()
}
fn typed_ref(value: pb::ObjectRef, kind: ObjectKind) -> Result<ObjectRef, ModelError> {
    let r = convert_ref(value)?;
    if r.kind != kind {
        return Err(ModelError::Reference(r.id));
    }
    Ok(r)
}
fn refs(values: Vec<pb::ObjectRef>, kind: ObjectKind) -> Result<Vec<ObjectRef>, ModelError> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .map(|v| {
            let r = typed_ref(v, kind)?;
            if !seen.insert(r.id) {
                return Err(ModelError::DuplicateId(r.id));
            }
            Ok(r)
        })
        .collect()
}
fn resource_id(value: Vec<u8>) -> Result<ResourceId, ModelError> {
    value
        .try_into()
        .map_err(|_| invalid("resource fingerprint must be 32 bytes"))
}
fn number(value: f64, limits: &Limits) -> Result<f64, ModelError> {
    if !value.is_finite()
        || value.abs() > limits.max_geometry
        || !limits.max_geometry.is_finite()
        || limits.max_geometry <= 0.0
    {
        return Err(invalid("nonfinite or excessive numeric value"));
    }
    Ok(if value == 0.0 { 0.0 } else { value })
}
fn finite_scalar(value: f64) -> Result<f64, ModelError> {
    if !value.is_finite() {
        return Err(invalid("nonfinite scalar"));
    }
    Ok(if value == 0.0 { 0.0 } else { value })
}
fn unit(value: f64) -> Result<f64, ModelError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid("invalid opacity/color"));
    }
    Ok(if value == 0.0 { 0.0 } else { value })
}
fn rect(value: pb::Rect, limits: &Limits) -> Result<Rect, ModelError> {
    let r = Rect {
        x: number(value.x, limits)?,
        y: number(value.y, limits)?,
        width: number(value.width, limits)?,
        height: number(value.height, limits)?,
    };
    if r.width < 0.0
        || r.height < 0.0
        || (r.x + r.width).abs() > limits.max_geometry
        || (r.y + r.height).abs() > limits.max_geometry
    {
        return Err(invalid("invalid rectangle bounds"));
    }
    Ok(r)
}
fn point(value: pb::Point, limits: &Limits) -> Result<Point, ModelError> {
    Ok(Point {
        x: number(value.x, limits)?,
        y: number(value.y, limits)?,
    })
}
fn affine(value: pb::Affine, limits: &Limits) -> Result<Affine, ModelError> {
    Ok(Affine {
        xx: number(value.xx, limits)?,
        yx: number(value.yx, limits)?,
        xy: number(value.xy, limits)?,
        yy: number(value.yy, limits)?,
        tx: number(value.tx, limits)?,
        ty: number(value.ty, limits)?,
    })
}
fn color(value: pb::Color) -> Result<Color, ModelError> {
    let c = Color {
        red: unit(value.red)?,
        green: unit(value.green)?,
        blue: unit(value.blue)?,
        alpha: unit(value.alpha)?,
    };
    if c.red > c.alpha || c.green > c.alpha || c.blue > c.alpha {
        return Err(invalid("color is not premultiplied"));
    }
    Ok(c)
}
fn string(value: String, limits: &Limits) -> Result<String, ModelError> {
    if value.len() > limits.max_string_bytes {
        return Err(ModelError::Limit("string"));
    }
    Ok(value)
}
fn required<T>(value: Option<T>, field: &str) -> Result<T, ModelError> {
    value.ok_or_else(|| invalid(field))
}

fn corner_size(value: pb::Size, limits: &Limits) -> Result<Size, ModelError> {
    let radius = Size {
        width: number(value.width, limits)?,
        height: number(value.height, limits)?,
    };
    if radius.width < 0.0
        || radius.height < 0.0
        || ((radius.width == 0.0) != (radius.height == 0.0))
    {
        return Err(invalid(
            "corner radii must be nonnegative paired zeros or paired positives",
        ));
    }
    Ok(radius)
}
fn check_radii(radii: CornerRadii, bounds: Rect) -> Result<(), ModelError> {
    if radii.top_left.width + radii.top_right.width > bounds.width
        || radii.bottom_left.width + radii.bottom_right.width > bounds.width
        || radii.top_left.height + radii.bottom_left.height > bounds.height
        || radii.top_right.height + radii.bottom_right.height > bounds.height
    {
        return Err(invalid("corner radii overlap adjacent edges"));
    }
    Ok(())
}
fn corner_radii(
    value: pb::CornerRadii,
    bounds: Rect,
    limits: &Limits,
) -> Result<CornerRadii, ModelError> {
    let radii = CornerRadii {
        top_left: corner_size(required(value.top_left, "top-left radius")?, limits)?,
        top_right: corner_size(required(value.top_right, "top-right radius")?, limits)?,
        bottom_right: corner_size(required(value.bottom_right, "bottom-right radius")?, limits)?,
        bottom_left: corner_size(required(value.bottom_left, "bottom-left radius")?, limits)?,
    };
    check_radii(radii, bounds)?;
    Ok(radii)
}
fn check_border(bounds: Rect, radii: CornerRadii, width: f64) -> Result<(), ModelError> {
    if width < 0.0 || width > bounds.width.min(bounds.height) / 2.0 {
        return Err(invalid("border width must fit inside its rectangle"));
    }
    let inset = |r: Size| {
        let rx = (r.width - width).max(0.0);
        let ry = (r.height - width).max(0.0);
        if rx == 0.0 || ry == 0.0 {
            Size {
                width: 0.0,
                height: 0.0,
            }
        } else {
            Size {
                width: rx,
                height: ry,
            }
        }
    };
    check_radii(
        CornerRadii {
            top_left: inset(radii.top_left),
            top_right: inset(radii.top_right),
            bottom_right: inset(radii.bottom_right),
            bottom_left: inset(radii.bottom_left),
        },
        Rect {
            x: bounds.x + width,
            y: bounds.y + width,
            width: bounds.width - 2.0 * width,
            height: bounds.height - 2.0 * width,
        },
    )
}
fn gradient_stops(
    values: Vec<pb::GradientStop>,
    limits: &Limits,
) -> Result<Vec<GradientStop>, ModelError> {
    if values.len() > limits.max_gradient_stops {
        return Err(ModelError::Limit("gradient stops"));
    }
    if values.len() < 2 {
        return Err(invalid("gradient needs at least two stops"));
    }
    let stops = values
        .into_iter()
        .map(|v| {
            Ok(GradientStop {
                offset: unit(v.offset)?,
                color: color(required(v.color, "gradient stop color")?)?,
            })
        })
        .collect::<Result<Vec<_>, ModelError>>()?;
    if stops.first().unwrap().offset != 0.0
        || stops.last().unwrap().offset != 1.0
        || stops.windows(2).any(|v| v[0].offset > v[1].offset)
    {
        return Err(invalid(
            "gradient stops must be ordered and cover zero through one",
        ));
    }
    Ok(stops)
}
fn stroke_dash(values: Vec<f64>, offset: f64, limits: &Limits) -> Result<Vec<f64>, ModelError> {
    if values.len() > limits.max_dash_segments {
        return Err(ModelError::Limit("stroke dash segments"));
    }
    if !values.len().is_multiple_of(2) {
        return Err(invalid(
            "dash pattern must contain an even number of lengths",
        ));
    }
    let lengths = values
        .into_iter()
        .map(|v| {
            let length = number(v, limits)?;
            if length <= 0.0 {
                return Err(invalid("dash lengths must be positive"));
            }
            Ok(length)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let period = lengths.iter().sum::<f64>();
    if !period.is_finite()
        || period > limits.max_geometry
        || offset < 0.0
        || (lengths.is_empty() && offset != 0.0)
        || (!lengths.is_empty() && offset >= period)
    {
        return Err(invalid("dash period/offset must be bounded and normalized"));
    }
    Ok(lengths)
}

fn convert_record(value: pb::Record, limits: &Limits) -> Result<Record, ModelError> {
    let record = match value.record.ok_or(ModelError::Unsupported)? {
        pb::record::Record::Surface(v) => {
            let coverage = VisualCoverage::from_wire(v.visual_coverage)?;
            let consistency = CaptureConsistency::from_wire(v.capture_consistency)?;
            if v.source_capture_tokens.iter().any(|v| v.is_empty())
                || v.source_capture_tokens.is_empty()
                || (consistency == CaptureConsistency::Observed
                    && (v.source_capture_tokens.len() < 2
                        || v.source_capture_tokens[0] == v.source_capture_tokens[1]))
            {
                return Err(invalid("capture consistency lacks source tokens"));
            }
            if coverage == VisualCoverage::Complete
                && (v.scene_root.is_none() || !v.unsupported_features.is_empty())
            {
                return Err(invalid("complete surface requires a supported scene"));
            }
            if coverage != VisualCoverage::Complete && v.unsupported_features.is_empty() {
                return Err(invalid(
                    "coverage gap requires explicit unsupported features",
                ));
            }
            if matches!(
                SurfaceRole::from_wire(v.role)?,
                SurfaceRole::Popup | SurfaceRole::Tooltip
            ) && v.parent_surface.is_none()
            {
                return Err(invalid("transient surface requires a parent"));
            }
            let content = required(v.content_size, "content_size")?;
            let content_size = Size {
                width: number(content.width, limits)?,
                height: number(content.height, limits)?,
            };
            if content_size.width < 0.0 || content_size.height < 0.0 {
                return Err(invalid("negative content size"));
            }
            if v.routing_token.len() > limits.max_string_bytes
                || v.source_capture_tokens
                    .iter()
                    .any(|v| v.len() > limits.max_string_bytes)
            {
                return Err(ModelError::Limit("capture/routing token"));
            }
            Record::Surface(Surface {
                id: v.id,
                version: v.version,
                role: SurfaceRole::from_wire(v.role)?,
                parent: optional_ref(v.parent_surface, ObjectKind::Surface)?,
                desktop_rect: rect(required(v.desktop_rect, "desktop_rect")?, limits)?,
                content_size,
                transform: affine(required(v.transform, "surface transform")?, limits)?,
                clip: v.clip.map(|v| rect(v, limits)).transpose()?,
                opacity: unit(v.opacity)?,
                stacking_order: v.stacking_order,
                mapped: v.mapped,
                focused: v.focused,
                modal_for: optional_ref(v.modal_for, ObjectKind::Surface)?,
                scene_root: optional_ref(v.scene_root, ObjectKind::Scene)?,
                semantic_root: optional_ref(v.semantic_root, ObjectKind::Semantic)?,
                input_region: v
                    .input_region
                    .into_iter()
                    .map(|v| rect(v, limits))
                    .collect::<Result<_, _>>()?,
                routing_token: v.routing_token,
                visual_coverage: coverage,
                semantic_coverage: SemanticCoverage::from_wire(v.semantic_coverage)?,
                capture_consistency: CaptureConsistency::from_wire(v.capture_consistency)?,
                source_capture_tokens: v.source_capture_tokens,
                unsupported_features: v
                    .unsupported_features
                    .into_iter()
                    .map(|v| string(v, limits))
                    .collect::<Result<_, _>>()?,
            })
        }
        pb::record::Record::Scene(v) => {
            let primitive = match v.primitive.ok_or(ModelError::Unsupported)? {
                pb::scene_node::Primitive::Group(_) => Primitive::Group,
                pb::scene_node::Primitive::SolidRect(v) => Primitive::SolidRect {
                    rect: rect(required(v.rect, "solid rect")?, limits)?,
                    color: color(required(v.color, "solid color")?)?,
                },
                pb::scene_node::Primitive::FillPath(v) => Primitive::FillPath {
                    resource: resource_id(v.resource)?,
                    color: color(required(v.color, "path color")?)?,
                    fill_rule: FillRule::from_wire(v.fill_rule)?,
                },
                pb::scene_node::Primitive::GlyphRun(v) => {
                    if v.glyph_ids.len() != v.positions.len()
                        || (!v.scalar_clusters.is_empty()
                            && v.scalar_clusters.len() != v.glyph_ids.len())
                    {
                        return Err(invalid("glyph positions/clusters length"));
                    }
                    let size = number(v.size, limits)?;
                    if size <= 0.0 {
                        return Err(invalid("glyph size"));
                    }
                    let font = match v.font.ok_or(ModelError::Unsupported)? {
                        pb::glyph_run::Font::FontFingerprint(v) => {
                            GlyphFont::Fingerprint(resource_id(v)?)
                        }
                        pb::glyph_run::Font::OutlineResource(v) => {
                            GlyphFont::Outline(resource_id(v)?)
                        }
                    };
                    Primitive::GlyphRun {
                        font,
                        size,
                        glyph_ids: v.glyph_ids,
                        positions: v
                            .positions
                            .into_iter()
                            .map(|v| point(v, limits))
                            .collect::<Result<_, _>>()?,
                        scalar_clusters: v.scalar_clusters,
                        color: color(required(v.color, "glyph color")?)?,
                    }
                }
                pb::scene_node::Primitive::RoundedRect(v) => {
                    let bounds = rect(required(v.rect, "rounded rectangle")?, limits)?;
                    Primitive::RoundedRect {
                        rect: bounds,
                        radii: corner_radii(required(v.radii, "corner radii")?, bounds, limits)?,
                        color: color(required(v.color, "rounded color")?)?,
                    }
                }
                pb::scene_node::Primitive::Border(v) => {
                    let bounds = rect(required(v.rect, "border rectangle")?, limits)?;
                    let radii =
                        corner_radii(required(v.radii, "border corner radii")?, bounds, limits)?;
                    let width = number(v.width, limits)?;
                    check_border(bounds, radii, width)?;
                    Primitive::Border {
                        rect: bounds,
                        radii,
                        width,
                        color: color(required(v.color, "border color")?)?,
                    }
                }
                pb::scene_node::Primitive::LinearGradient(v) => {
                    let start = point(required(v.start, "gradient start")?, limits)?;
                    let end = point(required(v.end, "gradient end")?, limits)?;
                    let span = (end.x - start.x).hypot(end.y - start.y);
                    if !span.is_finite() || span == 0.0 {
                        return Err(invalid("linear gradient requires a finite nonzero span"));
                    }
                    Primitive::LinearGradient {
                        rect: rect(required(v.rect, "gradient rectangle")?, limits)?,
                        start,
                        end,
                        stops: gradient_stops(v.stops, limits)?,
                        spread: GradientSpread::from_wire(v.spread)?,
                    }
                }
                pb::scene_node::Primitive::RadialGradient(v) => {
                    let center = point(required(v.center, "radial center")?, limits)?;
                    let inner_radius = number(v.inner_radius, limits)?;
                    let outer_radius = number(v.outer_radius, limits)?;
                    if inner_radius < 0.0
                        || inner_radius >= outer_radius
                        || [
                            center.x - outer_radius,
                            center.x + outer_radius,
                            center.y - outer_radius,
                            center.y + outer_radius,
                        ]
                        .iter()
                        .any(|v| !v.is_finite() || v.abs() > limits.max_geometry)
                    {
                        return Err(invalid(
                            "radial circles require bounded center and zero<=inner<outer",
                        ));
                    }
                    Primitive::RadialGradient {
                        rect: rect(required(v.rect, "radial rectangle")?, limits)?,
                        center,
                        inner_radius,
                        outer_radius,
                        stops: gradient_stops(v.stops, limits)?,
                        spread: GradientSpread::from_wire(v.spread)?,
                    }
                }
                pb::scene_node::Primitive::StrokePath(v) => {
                    let width = number(v.width, limits)?;
                    let cap = StrokeCap::from_wire(v.cap)?;
                    let join = StrokeJoin::from_wire(v.join)?;
                    let miter_limit = finite_scalar(v.miter_limit)?;
                    if width <= 0.0
                        || !limits.max_stroke_miter_limit.is_finite()
                        || limits.max_stroke_miter_limit < 1.0
                        || miter_limit < 1.0
                        || miter_limit > limits.max_stroke_miter_limit
                        || (join != StrokeJoin::Miter && miter_limit != 1.0)
                        || !(width * miter_limit).is_finite()
                    {
                        return Err(invalid("stroke width or miter limit"));
                    }
                    let dash_offset = number(v.dash_offset, limits)?;
                    Primitive::StrokePath {
                        resource: resource_id(v.resource)?,
                        color: color(required(v.color, "stroke color")?)?,
                        width,
                        cap,
                        join,
                        miter_limit,
                        dash_lengths: stroke_dash(v.dash_lengths, dash_offset, limits)?,
                        dash_offset,
                    }
                }
            };
            if !matches!(primitive, Primitive::Group) && !v.ordered_children.is_empty() {
                return Err(invalid(
                    "only Group owns scene children in initial vocabulary",
                ));
            }
            Record::Scene(SceneNode {
                id: v.id,
                version: v.version,
                children: refs(v.ordered_children, ObjectKind::Scene)?,
                transform: affine(required(v.transform, "scene transform")?, limits)?,
                clip: v.clip.map(|v| rect(v, limits)).transpose()?,
                opacity: unit(v.opacity)?,
                primitive,
                resources: v
                    .resource_refs
                    .into_iter()
                    .map(resource_id)
                    .collect::<Result<_, _>>()?,
            })
        }
        pb::record::Record::Semantic(v) => {
            let mut states = BTreeMap::new();
            for state in v.states {
                let key = StateKey::from_wire(state.key)?;
                if states
                    .insert(key, TriState::from_wire(state.value)?)
                    .is_some()
                {
                    return Err(invalid("duplicate semantic state"));
                }
            }
            let value = v
                .typed_value
                .map(|v| -> Result<TypedValue, ModelError> {
                    match v.value.ok_or(ModelError::Unsupported)? {
                        pb::typed_value::Value::Text(v) => Ok(TypedValue::Text(string(v, limits)?)),
                        pb::typed_value::Value::Boolean(v) => Ok(TypedValue::Boolean(v)),
                        pb::typed_value::Value::Number(v) => {
                            Ok(TypedValue::Number(number(v, limits)?))
                        }
                        pb::typed_value::Value::Integer(v) => Ok(TypedValue::Integer(v)),
                    }
                })
                .transpose()?;
            let collection = v
                .collection_model
                .map(|c| {
                    if c.version == 0 {
                        return Err(invalid("collection version"));
                    }
                    let items = refs(c.items, ObjectKind::Semantic)?;
                    let selected_items = refs(c.selected_items, ObjectKind::Semantic)?;
                    let materialization = Materialization::from_wire(c.materialization)?;
                    if c.total_count.is_some_and(|n| n < items.len() as u64)
                        || (materialization == Materialization::Full
                            && c.total_count.is_some_and(|n| n != items.len() as u64))
                        || selected_items.iter().any(|r| !items.contains(r))
                    {
                        return Err(invalid("collection counts/selection"));
                    }
                    Ok(CollectionModel {
                        version: c.version,
                        items,
                        total_count: c.total_count,
                        materialization,
                        selected_items,
                    })
                })
                .transpose()?;
            let mut action_ids = BTreeSet::new();
            let actions = v
                .actions
                .into_iter()
                .map(|a| {
                    if a.schema_version == 0 || a.id.is_empty() || !action_ids.insert(a.id.clone())
                    {
                        return Err(invalid("action vocabulary/version/duplicate"));
                    }
                    Ok(Action {
                        id: string(a.id, limits)?,
                        schema_version: a.schema_version,
                    })
                })
                .collect::<Result<_, _>>()?;
            let node = SemanticNode {
                browser_input_type: v
                    .browser_input_type
                    .map(|v| string(v, limits))
                    .transpose()?,
                id: v.id,
                version: v.version,
                role: SemanticRole::from_wire(v.role)?,
                label: v.label.map(|v| string(v, limits)).transpose()?,
                description: v.description.map(|v| string(v, limits)).transpose()?,
                value,
                states,
                children: refs(v.ordered_children, ObjectKind::Semantic)?,
                relations: v
                    .relations
                    .into_iter()
                    .map(|r| {
                        Ok(Relation {
                            kind: RelationKind::from_wire(r.kind)?,
                            targets: refs(r.targets, ObjectKind::Semantic)?,
                        })
                    })
                    .collect::<Result<_, ModelError>>()?,
                actions,
                text: optional_ref(v.text_model, ObjectKind::Text)?,
                collection,
                bindings: refs(v.visual_bindings, ObjectKind::Binding)?,
                semantic_group: optional_ref(v.semantic_group, ObjectKind::Semantic)?,
                reading_order: v.reading_order,
                sensitivity: Sensitivity::from_wire(v.sensitivity)?,
                completeness: Completeness::from_wire(v.completeness)?,
            };
            if node.sensitivity == Sensitivity::Redacted
                && (node.label.is_some() || node.description.is_some() || node.value.is_some())
            {
                return Err(invalid("redacted semantic record includes private content"));
            }
            if node
                .collection
                .as_ref()
                .is_some_and(|c| c.items.iter().any(|i| !node.children.contains(i)))
            {
                return Err(invalid("collection items must be owned children"));
            }
            Record::Semantic(node)
        }
        pb::record::Record::Text(v) => {
            let status = TextStatus::from_wire(v.status)?;
            if v.text_version == 0 {
                return Err(invalid("zero text version"));
            }
            if (status == TextStatus::Available) != v.content.is_some()
                || (status != TextStatus::Available && (v.selection.is_some() || v.editable))
            {
                return Err(invalid("text status/content mismatch"));
            }
            let content = v.content.map(|v| string(v, limits)).transpose()?;
            let count = content.as_ref().map_or(0, |v| v.chars().count() as u64);
            let selection = v.selection.map(|r| TextRange {
                anchor: r.anchor,
                focus: r.focus,
            });
            if selection.is_some_and(|r| r.anchor > count || r.focus > count)
                || (selection.is_some() && !v.selectable)
            {
                return Err(invalid("scalar-indexed selection out of range"));
            }
            Record::Text(TextModel {
                id: v.id,
                version: v.version,
                text_version: v.text_version,
                status,
                content,
                selection,
                editable: v.editable,
                selectable: v.selectable,
            })
        }
        pb::record::Record::Binding(v) => Record::Binding(Binding {
            id: v.id,
            version: v.version,
            surface: typed_ref(required(v.surface, "binding surface")?, ObjectKind::Surface)?,
            semantics: refs(v.semantic_nodes, ObjectKind::Semantic)?,
            scenes: refs(v.scene_nodes, ObjectKind::Scene)?,
            interaction_region: v.interaction_region.map(|v| rect(v, limits)).transpose()?,
        }),
        pb::record::Record::Session(v) => {
            let cursor = v
                .cursor
                .map(|v| -> Result<Cursor, ModelError> {
                    let appearance = match v.appearance.ok_or(ModelError::Unsupported)? {
                        pb::cursor::Appearance::Shape(v) => {
                            CursorAppearance::Shape(CursorShape::from_wire(v)?)
                        }
                        pb::cursor::Appearance::OutlineResource(v) => {
                            CursorAppearance::Outline(resource_id(v)?)
                        }
                    };
                    Ok(Cursor {
                        position: point(required(v.position, "cursor position")?, limits)?,
                        hotspot: point(required(v.hotspot, "cursor hotspot")?, limits)?,
                        appearance,
                    })
                })
                .transpose()?;
            Record::Session(SessionState {
                id: v.id,
                version: v.version,
                canvas: rect(required(v.canvas, "session canvas")?, limits)?,
                cursor,
                focus: optional_ref(v.focus, ObjectKind::Semantic)?,
                focus_surface: optional_ref(v.focus_surface, ObjectKind::Surface)?,
                focus_generation: v.focus_generation,
            })
        }
    };
    identity(record.id(), record.version())?;
    Ok(record)
}

fn validate_graph(state: &State, limits: &Limits) -> Result<(), ModelError> {
    let resolve = |r: ObjectRef| -> Result<(), ModelError> {
        match state.records.get(&r.id) {
            Some(record) if record.kind() == r.kind => Ok(()),
            _ => Err(ModelError::Reference(r.id)),
        }
    };
    let mut scene_owners = BTreeMap::<u64, u64>::new();
    let mut semantic_owners = BTreeMap::<u64, u64>::new();
    let mut scene_roots = BTreeSet::new();
    let mut semantic_roots = BTreeSet::new();
    let mut sessions = 0;
    let mut surfaces = 0;
    let mut scene_surfaces = BTreeMap::new();
    let mut semantic_surfaces = BTreeMap::new();
    for record in state.records.values() {
        match record {
            Record::Surface(v) => {
                surfaces += 1;
                for r in [v.parent, v.modal_for, v.scene_root, v.semantic_root]
                    .into_iter()
                    .flatten()
                {
                    resolve(r)?;
                }
                if v.parent.is_some_and(|r| r.id == v.id)
                    || v.modal_for.is_some_and(|r| r.id == v.id)
                {
                    return Err(ModelError::Ownership(v.id));
                }
                if let Some(r) = v.scene_root {
                    if !scene_roots.insert(r.id) {
                        return Err(ModelError::Ownership(r.id));
                    }
                    scene_surfaces.insert(r.id, v.id);
                }
                if let Some(r) = v.semantic_root {
                    if !semantic_roots.insert(r.id) {
                        return Err(ModelError::Ownership(r.id));
                    }
                    semantic_surfaces.insert(r.id, v.id);
                }
            }
            Record::Scene(v) => {
                for r in &v.children {
                    resolve(*r)?;
                    if scene_owners.insert(r.id, v.id).is_some() {
                        return Err(ModelError::Ownership(r.id));
                    }
                }
            }
            Record::Semantic(v) => {
                for r in &v.children {
                    resolve(*r)?;
                    if semantic_owners.insert(r.id, v.id).is_some() {
                        return Err(ModelError::Ownership(r.id));
                    }
                }
                for r in v
                    .relations
                    .iter()
                    .flat_map(|r| &r.targets)
                    .chain(&v.bindings)
                    .chain(v.text.iter())
                    .chain(v.semantic_group.iter())
                {
                    resolve(*r)?;
                }
                for binding_ref in &v.bindings {
                    if let Some(Record::Binding(binding)) = state.records.get(&binding_ref.id) {
                        if !binding.semantics.iter().any(|r| r.id == v.id) {
                            return Err(invalid("binding membership is not reciprocal"));
                        }
                    }
                }
            }
            Record::Binding(v) => {
                resolve(v.surface)?;
                for r in v.semantics.iter().chain(&v.scenes) {
                    resolve(*r)?;
                }
                for r in &v.semantics {
                    if let Some(Record::Semantic(node)) = state.records.get(&r.id) {
                        if !node.bindings.iter().any(|b| b.id == v.id) {
                            return Err(invalid("binding membership is not reciprocal"));
                        }
                    }
                }
            }
            Record::Session(v) => {
                sessions += 1;
                for r in v.focus.iter().chain(v.focus_surface.iter()) {
                    resolve(*r)?;
                }
            }
            Record::Text(_) => {}
        }
    }
    if sessions > 1 {
        return Err(invalid("more than one session record"));
    }
    for id in &scene_roots {
        if scene_owners.contains_key(id) {
            return Err(ModelError::Ownership(*id));
        }
    }
    for id in &semantic_roots {
        if semantic_owners.contains_key(id) {
            return Err(ModelError::Ownership(*id));
        }
    }
    let scene_index = ownership_index(
        state
            .records
            .values()
            .filter(|r| {
                r.kind() == ObjectKind::Scene
                    && (surfaces > 0 || limits.max_depth == 0 || scene_owners.contains_key(&r.id()))
            })
            .map(Record::id),
        &scene_owners,
        limits.max_depth,
    )?;
    let semantic_index = ownership_index(
        state
            .records
            .values()
            .filter(|r| {
                r.kind() == ObjectKind::Semantic
                    && (surfaces > 0
                        || limits.max_depth == 0
                        || semantic_owners.contains_key(&r.id()))
            })
            .map(Record::id),
        &semantic_owners,
        limits.max_depth,
    )?;
    let surface_owners = state
        .records
        .values()
        .filter_map(|r| match r {
            Record::Surface(s) => s.parent.map(|parent| (s.id, parent.id)),
            _ => None,
        })
        .collect();
    ownership_index(
        state
            .records
            .values()
            .filter(|r| r.kind() == ObjectKind::Surface)
            .map(Record::id),
        &surface_owners,
        limits.max_depth,
    )?;
    if surfaces > 0 {
        for (index, roots) in [
            (&scene_index, &scene_roots),
            (&semantic_index, &semantic_roots),
        ] {
            for (id, (root, _)) in index {
                if !roots.contains(root) {
                    return Err(ModelError::Ownership(*id));
                }
            }
        }
    }
    let surface_of = |r: ObjectRef| -> Option<u64> {
        let (index, roots) = match r.kind {
            ObjectKind::Scene => (&scene_index, &scene_surfaces),
            ObjectKind::Semantic => (&semantic_index, &semantic_surfaces),
            _ => return None,
        };
        index
            .get(&r.id)
            .and_then(|(root, _)| roots.get(root))
            .copied()
    };
    for record in state.records.values() {
        match record {
            Record::Binding(binding) => {
                if binding
                    .scenes
                    .iter()
                    .chain(&binding.semantics)
                    .any(|r| surface_of(*r) != Some(binding.surface.id))
                {
                    return Err(invalid("binding crosses surface ownership"));
                }
            }
            Record::Session(session) if surfaces > 0 => {
                if let Some(focus) = session.focus {
                    if surface_of(focus) != session.focus_surface.map(|r| r.id) {
                        return Err(invalid("focus surface does not own focused node"));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn ownership_index(
    ids: impl Iterator<Item = u64>,
    parents: &BTreeMap<u64, u64>,
    max_depth: usize,
) -> Result<BTreeMap<u64, (u64, usize)>, ModelError> {
    let mut index = BTreeMap::<u64, (u64, usize)>::new();
    let mut path = Vec::new();
    let mut visiting = BTreeSet::new();
    for id in ids {
        let mut at = id;
        let (root, mut depth) = loop {
            if let Some(value) = index.get(&at) {
                break *value;
            }
            if !visiting.insert(at) {
                return Err(ModelError::Ownership(at));
            }
            path.push(at);
            if path.len() > max_depth {
                return Err(ModelError::Limit("ownership depth"));
            }
            match parents.get(&at) {
                Some(parent) => at = *parent,
                None => break (at, 0),
            }
        };
        while let Some(id) = path.pop() {
            depth += 1;
            if depth > max_depth {
                return Err(ModelError::Limit("ownership depth"));
            }
            visiting.remove(&id);
            index.insert(id, (root, depth));
        }
    }
    Ok(index)
}
