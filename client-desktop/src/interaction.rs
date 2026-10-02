//! Generic interaction with the published view. It never identifies application
//! controls by labels, fixture IDs or client-side business rules.
use crate::Viewport;
use rui_protocol_core::commands::{Arguments, Id, Preconditions, SemanticCommand};
use rui_protocol_core::model::*;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InteractionError {
    #[error("view changed before input was dispatched")]
    StalePresentation,
    #[error("no unambiguous enabled remote target at this position")]
    NoTarget,
    #[error("remote action is unavailable or arguments are unsupported")]
    Unsupported,
    #[error("control has not been granted")]
    NoControl,
    #[error("invalid presentation geometry")]
    Geometry,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub surface: u64,
    pub semantic: u64,
}
fn contains(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.x < r.x + r.width && p.y >= r.y && p.y < r.y + r.height
}
fn collection_version(state: &State, node: &SemanticNode) -> Result<Option<u64>, InteractionError> {
    if let Some(collection) = &node.collection {
        return Ok(Some(collection.version));
    }
    let mut owners = state.records.values().filter_map(|record| match record {
        Record::Semantic(owner) => owner.collection.as_ref().filter(|collection| {
            collection.items.contains(&ObjectRef {
                kind: ObjectKind::Semantic,
                id: node.id,
            })
        }),
        _ => None,
    });
    let version = owners.next().map(|collection| collection.version);
    if owners.next().is_some() || (node.role == SemanticRole::ListItem && version.is_none()) {
        return Err(InteractionError::Unsupported);
    }
    Ok(version)
}
fn local(surface: &Surface, p: Point) -> Option<Point> {
    let m = surface.transform;
    let determinant = m.xx * m.yy - m.xy * m.yx;
    if determinant == 0.0 || !determinant.is_finite() {
        return None;
    }
    let x = p.x - surface.desktop_rect.x - m.tx;
    let y = p.y - surface.desktop_rect.y - m.ty;
    let p = Point {
        x: (m.yy * x - m.xy * y) / determinant,
        y: (m.xx * y - m.yx * x) / determinant,
    };
    if !p.x.is_finite() || !p.y.is_finite() {
        return None;
    }
    Some(p)
}
/// Regions are explicitly surface-content coordinates. Device coordinates use
/// the exact viewport that GTK submitted; local zoom/pan has no host side effect.
pub fn hit_test(
    state: &State,
    presented_revision: u64,
    viewport: Viewport,
    device: Point,
) -> Result<Target, InteractionError> {
    if state.revision != presented_revision || presented_revision == 0 {
        return Err(InteractionError::StalePresentation);
    }
    if !device.x.is_finite()
        || !device.y.is_finite()
        || !viewport.scale.is_finite()
        || viewport.scale <= 0.0
        || device.x < 0.0
        || device.y < 0.0
        || device.x >= viewport.width as f64
        || device.y >= viewport.height as f64
    {
        return Err(InteractionError::Geometry);
    }
    let p = Point {
        x: viewport.origin.x + device.x / viewport.scale,
        y: viewport.origin.y + device.y / viewport.scale,
    };
    if !p.x.is_finite() || !p.y.is_finite() {
        return Err(InteractionError::Geometry);
    }
    for record in state.records.values() {
        if let Record::Session(session) = record {
            if !contains(session.canvas, p) {
                return Err(InteractionError::NoTarget);
            }
        }
    }
    let surface = state
        .records
        .values()
        .filter_map(|record| match record {
            Record::Surface(surface)
                if surface.mapped && surface.visual_coverage == VisualCoverage::Complete =>
            {
                Some(surface)
            }
            _ => None,
        })
        .filter(|surface| {
            local(surface, p).is_some_and(|p| {
                contains(
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        width: surface.content_size.width,
                        height: surface.content_size.height,
                    },
                    p,
                ) && surface.clip.is_none_or(|clip| contains(clip, p))
                    && surface
                        .input_region
                        .iter()
                        .any(|region| contains(*region, p))
            })
        })
        .max_by_key(|surface| (surface.stacking_order, surface.id))
        .ok_or(InteractionError::NoTarget)?;
    if state.records.values().any(|record|matches!(record,Record::Surface(other) if other.mapped && other.modal_for==Some(ObjectRef{kind:ObjectKind::Surface,id:surface.id}))) {return Err(InteractionError::NoTarget);}
    let p = local(surface, p).ok_or(InteractionError::Geometry)?;
    let mut target = None;
    for record in state.records.values() {
        if let Record::Binding(binding) = record {
            if binding.surface
                != (ObjectRef {
                    kind: ObjectKind::Surface,
                    id: surface.id,
                })
                || binding.interaction_region.is_none_or(|r| !contains(r, p))
            {
                continue;
            }
            for reference in &binding.semantics {
                if reference.kind != ObjectKind::Semantic {
                    continue;
                }
                let Some(Record::Semantic(node)) = state.records.get(&reference.id) else {
                    continue;
                };
                if node.actions.is_empty()
                    || node
                        .states
                        .get(&StateKey::Disabled)
                        .is_some_and(|s| *s != TriState::False)
                {
                    continue;
                }
                let candidate = Target {
                    surface: surface.id,
                    semantic: node.id,
                };
                if target.is_some_and(|existing| existing != candidate) {
                    return Err(InteractionError::NoTarget);
                }
                target = Some(candidate);
            }
        }
    }
    target.ok_or(InteractionError::NoTarget)
}

/// Fresh authority and sequence are supplied by the connection actor, never
/// inferred from a semantic node or mirror pointer event.
pub struct CommandBinding {
    pub epoch: Id,
    pub view: Id,
    pub client: Id,
    pub lease: Id,
    pub generation: u64,
    pub sequence: u64,
}
/// An immutable remote editing context. A local IME never retargets a commit
/// after focus, selection, source versions or authority have changed.
#[derive(Clone, Copy, Debug)]
pub struct EditingContext {
    pub epoch: Id,
    pub view: Id,
    pub lease: Id,
    pub generation: u64,
    pub revision: u64,
    pub target: u64,
    pub target_version: u64,
    pub text: u64,
    pub text_record_version: u64,
    pub text_version: u64,
    pub focus_generation: u64,
    pub selection: TextRange,
}
impl PartialEq for EditingContext {
    fn eq(&self, other: &Self) -> bool {
        (
            self.epoch,
            self.view,
            self.lease,
            self.generation,
            self.target,
            self.target_version,
            self.text,
            self.text_record_version,
            self.text_version,
            self.focus_generation,
            self.selection,
        ) == (
            other.epoch,
            other.view,
            other.lease,
            other.generation,
            other.target,
            other.target_version,
            other.text,
            other.text_record_version,
            other.text_version,
            other.focus_generation,
            other.selection,
        )
    }
}
impl Eq for EditingContext {}
pub fn editing_context(
    state: &State,
    target: u64,
    authority: &CommandBinding,
) -> Option<EditingContext> {
    if authority.lease == [0; 16] || authority.generation == 0 {
        return None;
    }
    let Record::Semantic(node) = state.records.get(&target)? else {
        return None;
    };
    if !node
        .actions
        .iter()
        .any(|a| a.id == "ReplaceText" && a.schema_version == 1)
        || node
            .states
            .get(&StateKey::Disabled)
            .is_some_and(|s| *s != TriState::False)
    {
        return None;
    }
    let reference = node.text?;
    let Record::Text(text) = state.records.get(&reference.id)? else {
        return None;
    };
    if reference.kind != ObjectKind::Text || text.status != TextStatus::Available || !text.editable
    {
        return None;
    }
    let selection = text.selection?;
    let focus_generation = state.records.values().find_map(|r| match r {
        Record::Session(s)
            if s.focus
                == Some(ObjectRef {
                    kind: ObjectKind::Semantic,
                    id: target,
                })
                && s.focus_generation != 0 =>
        {
            Some(s.focus_generation)
        }
        _ => None,
    })?;
    Some(EditingContext {
        epoch: authority.epoch,
        view: authority.view,
        lease: authority.lease,
        generation: authority.generation,
        revision: state.revision,
        target,
        target_version: node.version,
        text: text.id,
        text_record_version: text.version,
        text_version: text.text_version,
        focus_generation,
        selection,
    })
}

/// Pure local IME state. Invalidation precedes GTK reset, which may emit signals.
/// Only one commit can use a context; new source evidence must unlock another.
#[derive(Default)]
pub struct LocalComposition {
    current: Option<EditingContext>,
    captured: Option<EditingContext>,
    blocked: bool,
}
impl LocalComposition {
    pub fn current(&self) -> Option<EditingContext> {
        self.current
    }
    pub fn invalidate(&mut self) {
        self.current = None;
        self.captured = None;
        self.blocked = true;
    }
    pub fn install(&mut self, context: Option<EditingContext>) {
        self.current = context;
        self.captured = None;
        self.blocked = false;
    }
    pub fn begin(&mut self) {
        if !self.blocked {
            self.captured = self.current;
        }
    }
    pub fn take_commit(&mut self) -> Option<EditingContext> {
        if self.blocked {
            return None;
        }
        self.blocked = true;
        self.captured.take()
    }
}

pub fn captured_text_command(
    state: &State,
    captured: EditingContext,
    text: String,
    binding: CommandBinding,
) -> Result<SemanticCommand, InteractionError> {
    if editing_context(state, captured.target, &binding) != Some(captured) {
        return Err(InteractionError::StalePresentation);
    }
    let mut command = command(
        state,
        captured.target,
        "ReplaceText",
        Arguments::ReplaceText {
            start: captured.selection.anchor.min(captured.selection.focus),
            end: captured.selection.anchor.max(captured.selection.focus),
            text,
        },
        binding,
    )?;
    command.observed_view_revision = captured.revision;
    Ok(command)
}
pub fn command(
    state: &State,
    target: u64,
    action: &str,
    args: Arguments,
    binding: CommandBinding,
) -> Result<SemanticCommand, InteractionError> {
    if binding.lease == [0; 16] || binding.generation == 0 || binding.sequence == 0 {
        return Err(InteractionError::NoControl);
    }
    let Some(Record::Semantic(node)) = state.records.get(&target) else {
        return Err(InteractionError::NoTarget);
    };
    if node
        .states
        .get(&StateKey::Disabled)
        .is_some_and(|s| *s != TriState::False)
    {
        return Err(InteractionError::NoTarget);
    }
    let action = node
        .actions
        .iter()
        .find(|a| a.id == action && a.schema_version == 1)
        .ok_or(InteractionError::Unsupported)?;
    let mut preconditions = Preconditions {
        target_version: Some(node.version),
        ..Default::default()
    };
    match (&*action.id, &args) {
        ("Activate", Arguments::None) => {
            preconditions.model_version = collection_version(state, node)?;
        }
        ("Focus", Arguments::None) => {}
        ("SetValue", Arguments::Checked(_) | Arguments::Number(_) | Arguments::ValueText(_)) => {}
        ("ReplaceText", Arguments::ReplaceText { .. })
        | ("SetSelection", Arguments::Selection { .. }) => {
            let reference = node.text.ok_or(InteractionError::Unsupported)?;
            let Some(Record::Text(text)) = state.records.get(&reference.id) else {
                return Err(InteractionError::Unsupported);
            };
            if reference.kind != ObjectKind::Text
                || text.status != TextStatus::Available
                || (action.id == "ReplaceText" && !text.editable)
                || (action.id == "SetSelection" && !text.selectable)
            {
                return Err(InteractionError::Unsupported);
            }
            preconditions.text_version = Some(text.text_version);
            let focus = state
                .records
                .values()
                .find_map(|r| match r {
                    Record::Session(session)
                        if session.focus
                            == Some(ObjectRef {
                                kind: ObjectKind::Semantic,
                                id: target,
                            }) =>
                    {
                        Some(session.focus_generation)
                    }
                    _ => None,
                })
                .filter(|v| *v != 0)
                .ok_or(InteractionError::Unsupported)?;
            preconditions.focus_generation = Some(focus);
        }
        ("ScrollTo", Arguments::Scroll { x, y }) if x.is_finite() && y.is_finite() => {
            preconditions.model_version = node.collection.as_ref().map(|c| c.version);
        }
        _ => return Err(InteractionError::Unsupported),
    }
    Ok(SemanticCommand {
        epoch: binding.epoch,
        view_id: binding.view,
        client_instance_id: binding.client,
        control_lease_id: binding.lease,
        control_generation: binding.generation,
        command_seq: binding.sequence,
        target_id: target,
        action_id: action.id.clone(),
        action_schema_version: action.schema_version,
        observed_view_revision: state.revision,
        preconditions,
        arguments: args,
    })
}
