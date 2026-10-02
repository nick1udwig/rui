//! Bounded cooperating source state for the expanded controlled corpus.
//! Public editing is Unicode scalar based. Rendering gaps remain explicit until
//! a complete native source glyph provider is configured.
use crate::fixture::*;
use crate::Projection;
use rui_protocol_core::commands::{
    Arguments, Rejection, SemanticAuthority, SemanticCommand, TargetState, ValueSchema,
};
use rui_protocol_core::wire::proto as p;

pub const PUBLIC_TEXT_ID: u64 = 60;
pub const PUBLIC_TEXT_MODEL_ID: u64 = 61;
pub const CHECKBOX_ID: u64 = 62;
pub const LIST_ID: u64 = 63;
pub const REORDER_ID: u64 = 64;
pub const POPUP_TOGGLE_ID: u64 = 65;
pub const MODAL_TOGGLE_ID: u64 = 66;
pub const TOOLTIP_TOGGLE_ID: u64 = 67;
pub const CANVAS_ID: u64 = 68;
pub const SECOND_SURFACE_ID: u64 = 69;
pub const ITEM_IDS: [u64; 3] = [70, 71, 72];
const ROOT_SECOND: u64 = 73;
const SCENE_SECOND: u64 = 74;
const FIRST_SCENE: u64 = 100;
const FIRST_BINDING: u64 = 200;
const MAX_TRANSIENT_INCARNS: u64 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransientKind {
    Popup,
    Modal,
    Tooltip,
}
#[derive(Clone, Copy)]
struct Transient {
    id: u64,
    kind: TransientKind,
}
pub(crate) struct Corpus {
    text: String,
    pub text_clock: u64,
    selection: (u64, u64),
    checked: bool,
    order: [u64; 3],
    selected: Option<u64>,
    model_clock: u64,
    scroll: (f64, f64),
    pub focus: u64,
    pub focus_generation: u64,
    popup: Option<Transient>,
    modal: Option<Transient>,
    modal_return_focus: Option<u64>,
    tooltip: Option<Transient>,
    incarnation: u64,
    pub width: f64,
    pub height: f64,
}
impl Corpus {
    pub fn new(max_text_bytes: usize) -> Self {
        Self {
            text: if max_text_bytes >= "AéΩ אב".len() {
                "AéΩ אב".into()
            } else {
                String::new()
            },
            text_clock: 1,
            selection: (0, 0),
            checked: false,
            order: ITEM_IDS,
            selected: None,
            model_clock: 1,
            scroll: (0.0, 0.0),
            focus: PUBLIC_TEXT_ID,
            focus_generation: 1,
            popup: None,
            modal: None,
            modal_return_focus: None,
            tooltip: None,
            incarnation: 0,
            width: 640.0,
            height: 480.0,
        }
    }
    pub fn extend_authority(&self, authority: &mut SemanticAuthority, projection: &Projection) {
        if self.modal.is_some() {
            for target in authority.targets.values_mut() {
                target.enabled = false;
            }
        }
        let mut add = |id, actions: &[&str], text: bool, model: bool, value_schema| {
            authority.targets.insert(
                id,
                TargetState {
                    enabled: self.modal.is_none() || self.modal.is_some_and(|m| id == m.id + 2),
                    target_version: projection.records.get(&id).map_or(1, |s| s.version),
                    text_version: text.then_some(self.text_clock),
                    text_scalar_count: text.then_some(self.text.chars().count() as u64),
                    focus_generation: text.then_some(self.focus_generation),
                    model_version: model.then_some(self.model_clock),
                    value_schema,
                    actions: actions.iter().map(|name| ((*name).into(), 1)).collect(),
                },
            );
        };
        add(
            PUBLIC_TEXT_ID,
            &["Focus", "ReplaceText", "SetSelection"],
            true,
            false,
            None,
        );
        add(
            CHECKBOX_ID,
            &["SetValue"],
            false,
            false,
            Some(ValueSchema::Boolean),
        );
        add(LIST_ID, &["ScrollTo"], false, true, None);
        add(REORDER_ID, &["Activate"], false, false, None);
        for id in ITEM_IDS {
            add(id, &["Activate"], false, true, None);
        }
        for id in [POPUP_TOGGLE_ID, MODAL_TOGGLE_ID, TOOLTIP_TOGGLE_ID] {
            add(id, &["Activate"], false, false, None);
        }
        for transient in [self.popup, self.modal].into_iter().flatten() {
            add(transient.id + 2, &["Activate"], false, false, None);
        }
    }
    pub fn validate(&self, command: &SemanticCommand, max_text: usize) -> Result<(), Rejection> {
        if matches!(command.action_id.as_str(), "ReplaceText" | "SetSelection")
            && (self.focus != PUBLIC_TEXT_ID
                || command.preconditions.focus_generation != Some(self.focus_generation))
        {
            return Err(Rejection::StaleFocus);
        }
        if matches!(command.target_id, LIST_ID | 70..=72)
            && command.preconditions.model_version != Some(self.model_clock)
        {
            return Err(Rejection::StaleModel);
        }
        match (&command.arguments, command.target_id) {
            (Arguments::ReplaceText { start, end, text }, PUBLIC_TEXT_ID) => {
                let start = scalar_byte(&self.text, *start).ok_or(Rejection::InvalidArguments)?;
                let end = scalar_byte(&self.text, *end).ok_or(Rejection::InvalidArguments)?;
                if start
                    .checked_add(text.len())
                    .and_then(|n| n.checked_add(self.text.len() - end))
                    .is_none_or(|n| n > max_text)
                {
                    return Err(Rejection::InvalidArguments);
                }
            }
            (Arguments::Scroll { x, y }, LIST_ID)
                if *x < 0.0 || *x > 100.0 || *y < 0.0 || *y > 1000.0 =>
            {
                return Err(Rejection::InvalidArguments)
            }
            _ => {}
        }
        Ok(())
    }
    pub fn focus(&mut self, target: u64) -> Result<(), Rejection> {
        if self.focus != target {
            self.focus_generation = self
                .focus_generation
                .checked_add(1)
                .ok_or(Rejection::InvalidArguments)?;
            self.focus = target;
        }
        Ok(())
    }
    fn toggle(&mut self, kind: TransientKind) -> Result<(), Rejection> {
        let live = match kind {
            TransientKind::Popup => &mut self.popup,
            TransientKind::Modal => &mut self.modal,
            TransientKind::Tooltip => &mut self.tooltip,
        };
        if live.take().is_none() {
            if self.incarnation >= MAX_TRANSIENT_INCARNS {
                return Err(Rejection::InvalidArguments);
            }
            self.incarnation += 1;
            *live = Some(Transient {
                id: 10_000 + self.incarnation * 8,
                kind,
            });
        }
        Ok(())
    }
    fn toggle_modal(&mut self) -> Result<(), Rejection> {
        if self.modal.is_some() {
            self.focus(self.modal_return_focus.unwrap_or(PUBLIC_TEXT_ID))?;
            self.modal = None;
            self.modal_return_focus = None;
        } else {
            if self.incarnation >= MAX_TRANSIENT_INCARNS {
                return Err(Rejection::InvalidArguments);
            }
            let generation = self
                .focus_generation
                .checked_add(1)
                .ok_or(Rejection::InvalidArguments)?;
            let prior = self.focus;
            self.toggle(TransientKind::Modal)?;
            self.modal_return_focus = Some(prior);
            self.focus = self.modal.unwrap().id + 2;
            self.focus_generation = generation;
        }
        Ok(())
    }
    pub fn apply(&mut self, command: &SemanticCommand, max_text: usize) -> Result<(), Rejection> {
        self.validate(command, max_text)?;
        match (
            command.target_id,
            command.action_id.as_str(),
            &command.arguments,
        ) {
            (PUBLIC_TEXT_ID, "Focus", Arguments::None) => self.focus(PUBLIC_TEXT_ID)?,
            (PUBLIC_TEXT_ID, "ReplaceText", Arguments::ReplaceText { start, end, text }) => {
                let start_byte =
                    scalar_byte(&self.text, *start).ok_or(Rejection::InvalidArguments)?;
                let end_byte = scalar_byte(&self.text, *end).ok_or(Rejection::InvalidArguments)?;
                let next_clock = self
                    .text_clock
                    .checked_add(1)
                    .ok_or(Rejection::InvalidArguments)?;
                let mut replacement =
                    String::with_capacity(start_byte + text.len() + self.text.len() - end_byte);
                replacement.push_str(&self.text[..start_byte]);
                replacement.push_str(text);
                replacement.push_str(&self.text[end_byte..]);
                if replacement != self.text {
                    self.text = replacement;
                    self.text_clock = next_clock;
                }
                let cursor = start + text.chars().count() as u64;
                self.selection = (cursor, cursor);
            }
            (PUBLIC_TEXT_ID, "SetSelection", Arguments::Selection { anchor, focus }) => {
                self.selection = (*anchor, *focus)
            }
            (CHECKBOX_ID, "SetValue", Arguments::Checked(value)) => self.checked = *value,
            (REORDER_ID, "Activate", Arguments::None) => {
                let clock = self
                    .model_clock
                    .checked_add(1)
                    .ok_or(Rejection::InvalidArguments)?;
                self.order.rotate_left(1);
                self.model_clock = clock;
            }
            (LIST_ID, "ScrollTo", Arguments::Scroll { x, y }) => self.scroll = (*x, y.min(60.0)),
            (id, "Activate", Arguments::None) if ITEM_IDS.contains(&id) => {
                if self.selected != Some(id) {
                    self.model_clock = self
                        .model_clock
                        .checked_add(1)
                        .ok_or(Rejection::InvalidArguments)?;
                    self.selected = Some(id);
                }
            }
            (POPUP_TOGGLE_ID, "Activate", Arguments::None) => self.toggle(TransientKind::Popup)?,
            (MODAL_TOGGLE_ID, "Activate", Arguments::None) => self.toggle_modal()?,
            (TOOLTIP_TOGGLE_ID, "Activate", Arguments::None) => {
                self.toggle(TransientKind::Tooltip)?
            }
            (id, "Activate", Arguments::None) => {
                if self.popup.is_some_and(|t| t.id + 2 == id) {
                    self.popup = None;
                } else if self.modal.is_some_and(|t| t.id + 2 == id) {
                    self.toggle_modal()?;
                } else {
                    return Err(Rejection::UnsupportedAction);
                }
            }
            _ => return Err(Rejection::UnsupportedAction),
        }
        Ok(())
    }
    pub fn extend_records(
        &self,
        records: &mut Vec<p::Record>,
        projection: &Projection,
        commit: u64,
    ) {
        for record in records.iter_mut().filter_map(|r| r.record.as_mut()) {
            match record {
                p::record::Record::Surface(s) => {
                    s.focused = self.modal.is_none();
                    s.desktop_rect = Some(rect(0.0, 0.0, self.width, self.height));
                    s.content_size = Some(p::Size {
                        width: self.width,
                        height: self.height,
                    });
                    s.input_region = vec![rect(0.0, 0.0, self.width, self.height)];
                    s.visual_coverage = p::VisualCoverage::Partial as i32;
                    s.unsupported_features = vec!["source-glyph-provider-pending".into()];
                }
                p::record::Record::Session(s) => {
                    s.canvas = Some(rect(0.0, 0.0, self.width + 280.0, self.height.max(200.0)));
                    s.focus = Some(reference(p::ObjectKind::Semantic, self.focus));
                    s.focus_generation = self.focus_generation;
                    s.focus_surface = Some(reference(
                        p::ObjectKind::Surface,
                        self.modal
                            .filter(|m| self.focus == m.id + 2)
                            .map_or(SURFACE_ID, |m| m.id),
                    ));
                }
                p::record::Record::Semantic(n)
                    if self.modal.is_some() && (n.id == COUNTER_ID || n.id == SECRET_ID) =>
                {
                    n.states
                        .retain(|state| state.key != p::StateKey::Disabled as i32);
                    n.states.push(p::StateValue {
                        key: p::StateKey::Disabled as i32,
                        value: p::TriState::True as i32,
                    });
                }
                p::record::Record::Semantic(n)
                    if n.id == SECRET_ID
                        && projection.permissions.can_control
                        && projection.permissions.actions.contains("Focus") =>
                {
                    n.actions.push(action("Focus"))
                }
                _ => {}
            }
        }
        let controls = [
            (
                PUBLIC_TEXT_ID,
                p::SemanticRole::TextField,
                "Public text",
                rect(160.0, 8.0, 220.0, 32.0),
                vec!["Focus", "ReplaceText", "SetSelection"],
            ),
            (
                CHECKBOX_ID,
                p::SemanticRole::Checkbox,
                "Checked",
                rect(160.0, 52.0, 32.0, 32.0),
                vec!["SetValue"],
            ),
            (
                LIST_ID,
                p::SemanticRole::List,
                "Stable items",
                rect(8.0, 110.0, 220.0, 110.0),
                vec!["ScrollTo"],
            ),
            (
                REORDER_ID,
                p::SemanticRole::Button,
                "Reorder",
                rect(240.0, 110.0, 100.0, 32.0),
                vec!["Activate"],
            ),
            (
                POPUP_TOGGLE_ID,
                p::SemanticRole::Button,
                "Popup",
                rect(8.0, 240.0, 100.0, 32.0),
                vec!["Activate"],
            ),
            (
                MODAL_TOGGLE_ID,
                p::SemanticRole::Button,
                "Modal",
                rect(120.0, 240.0, 100.0, 32.0),
                vec!["Activate"],
            ),
            (
                TOOLTIP_TOGGLE_ID,
                p::SemanticRole::Button,
                "Tooltip",
                rect(232.0, 240.0, 100.0, 32.0),
                vec!["Activate"],
            ),
            (
                CANVAS_ID,
                p::SemanticRole::CustomCanvas,
                "Vector canvas",
                rect(360.0, 100.0, 240.0, 300.0),
                vec![],
            ),
        ];
        for (index, (id, role, label, region, actions)) in controls.into_iter().enumerate() {
            let scene_id = FIRST_SCENE + index as u64;
            let binding_id = FIRST_BINDING + index as u64;
            let enabled = self.modal.is_none();
            let mut n = node(id, role, label, enabled, projection, &actions);
            n.visual_bindings = vec![reference(p::ObjectKind::Binding, binding_id)];
            if id == PUBLIC_TEXT_ID {
                n.text_model = Some(reference(p::ObjectKind::Text, PUBLIC_TEXT_MODEL_ID));
            }
            if id == CHECKBOX_ID {
                n.typed_value = Some(p::TypedValue {
                    value: Some(p::typed_value::Value::Boolean(self.checked)),
                });
                n.states.push(p::StateValue {
                    key: p::StateKey::Checked as i32,
                    value: if self.checked {
                        p::TriState::True
                    } else {
                        p::TriState::False
                    } as i32,
                });
            }
            if id == LIST_ID {
                n.ordered_children = self
                    .order
                    .iter()
                    .map(|id| reference(p::ObjectKind::Semantic, *id))
                    .collect();
                n.collection_model = Some(p::CollectionModel {
                    version: self.model_clock,
                    items: n.ordered_children.clone(),
                    total_count: Some(10),
                    materialization: p::Materialization::PartiallyMaterialized as i32,
                    selected_items: self
                        .selected
                        .into_iter()
                        .map(|id| reference(p::ObjectKind::Semantic, id))
                        .collect(),
                });
            }
            records.push(wrap(p::record::Record::Semantic(n)));
            let color = if id == CHECKBOX_ID && self.checked {
                [0.1, 0.8, 0.2, 1.0]
            } else {
                [0.22, 0.26, 0.32, 1.0]
            };
            records.push(wrap(p::record::Record::Scene(scene(
                scene_id, 1, region, color,
            ))));
            let mut b = binding(binding_id, id, scene_id, region);
            if id == LIST_ID {
                b.interaction_region = Some(rect(220.0, 118.0, 8.0, 90.0));
            }
            records.push(wrap(p::record::Record::Binding(b)));
        }
        records.push(wrap(p::record::Record::Text(p::TextModel {
            id: PUBLIC_TEXT_MODEL_ID,
            version: 1,
            text_version: self.text_clock,
            status: p::TextStatus::Available as i32,
            content: Some(self.text.clone()),
            selection: Some(p::TextRange {
                anchor: self.selection.0,
                focus: self.selection.1,
            }),
            editable: projection.permissions.can_control
                && projection.permissions.actions.contains("ReplaceText"),
            selectable: true,
        })));
        for (index, id) in self.order.iter().enumerate() {
            let scene_id = 120 + (*id - ITEM_IDS[0]);
            let binding_id = 220 + (*id - ITEM_IDS[0]);
            let region = rect(
                16.0 - self.scroll.0,
                118.0 + index as f64 * 30.0 - self.scroll.1.min(60.0),
                200.0,
                24.0,
            );
            let mut n = node(
                *id,
                p::SemanticRole::ListItem,
                &format!("Item {}", id - ITEM_IDS[0] + 1),
                self.modal.is_none(),
                projection,
                &["Activate"],
            );
            n.states.push(p::StateValue {
                key: p::StateKey::Selected as i32,
                value: if self.selected == Some(*id) {
                    p::TriState::True
                } else {
                    p::TriState::False
                } as i32,
            });
            n.visual_bindings = vec![reference(p::ObjectKind::Binding, binding_id)];
            records.push(wrap(p::record::Record::Semantic(n)));
            let mut row_scene = scene(
                scene_id,
                1,
                region,
                if self.selected == Some(*id) {
                    [0.2, 0.5, 0.8, 1.0]
                } else {
                    [0.35, 0.35, 0.4, 1.0]
                },
            );
            row_scene.clip = Some(rect(8.0, 110.0, 220.0, 110.0));
            records.push(wrap(p::record::Record::Scene(row_scene)));
            let mut b = binding(binding_id, *id, scene_id, region);
            let top = region.y.max(110.0);
            let bottom = (region.y + region.height).min(220.0);
            let left = region.x.max(8.0);
            let right = (region.x + region.width).min(228.0);
            b.interaction_region =
                (bottom > top && right > left).then(|| rect(left, top, right - left, bottom - top));
            records.push(wrap(p::record::Record::Binding(b)));
        }
        for record in records.iter_mut().filter_map(|r| r.record.as_mut()) {
            match record {
                p::record::Record::Semantic(n) if n.id == SEMANTIC_ROOT_ID => {
                    n.ordered_children.extend(
                        [
                            PUBLIC_TEXT_ID,
                            CHECKBOX_ID,
                            LIST_ID,
                            REORDER_ID,
                            POPUP_TOGGLE_ID,
                            MODAL_TOGGLE_ID,
                            TOOLTIP_TOGGLE_ID,
                            CANVAS_ID,
                        ]
                        .into_iter()
                        .map(|id| reference(p::ObjectKind::Semantic, id)),
                    )
                }
                p::record::Record::Scene(n) if n.id == SCENE_ROOT_ID => n.ordered_children.extend(
                    (FIRST_SCENE..FIRST_SCENE + 8)
                        .chain(120..123)
                        .map(|id| reference(p::ObjectKind::Scene, id)),
                ),
                _ => {}
            }
        }
        records.push(wrap(p::record::Record::Surface(p::Surface {
            id: SECOND_SURFACE_ID,
            version: 1,
            role: p::SurfaceRole::Toplevel as i32,
            desktop_rect: Some(rect(self.width + 20.0, 0.0, 240.0, 200.0)),
            content_size: Some(p::Size {
                width: 240.0,
                height: 200.0,
            }),
            transform: Some(affine()),
            opacity: 1.0,
            mapped: true,
            scene_root: Some(reference(p::ObjectKind::Scene, SCENE_SECOND)),
            semantic_root: Some(reference(p::ObjectKind::Semantic, ROOT_SECOND)),
            routing_token: b"fixture-second-route-1".to_vec(),
            visual_coverage: p::VisualCoverage::Complete as i32,
            semantic_coverage: p::SemanticCoverage::Rich as i32,
            capture_consistency: p::CaptureConsistency::Coherent as i32,
            source_capture_tokens: vec![commit.to_be_bytes().to_vec()],
            ..Default::default()
        })));
        records.push(wrap(p::record::Record::Scene(scene(
            SCENE_SECOND,
            1,
            rect(0.0, 0.0, 240.0, 200.0),
            [0.08, 0.12, 0.2, 1.0],
        ))));
        records.push(wrap(p::record::Record::Semantic(node(
            ROOT_SECOND,
            p::SemanticRole::Root,
            "Second source window",
            true,
            projection,
            &[],
        ))));
        for transient in [self.popup, self.modal, self.tooltip].into_iter().flatten() {
            self.transient_records(records, projection, transient, commit);
        }
    }
    fn transient_records(
        &self,
        records: &mut Vec<p::Record>,
        projection: &Projection,
        transient: Transient,
        commit: u64,
    ) {
        let id = transient.id;
        let tooltip = transient.kind == TransientKind::Tooltip;
        let region = rect(0.0, 0.0, 140.0, 70.0);
        records.push(wrap(p::record::Record::Surface(p::Surface {
            id,
            version: 1,
            role: if tooltip {
                p::SurfaceRole::Tooltip
            } else {
                p::SurfaceRole::Popup
            } as i32,
            parent_surface: Some(reference(p::ObjectKind::Surface, SURFACE_ID)),
            modal_for: if transient.kind == TransientKind::Modal {
                Some(reference(p::ObjectKind::Surface, SURFACE_ID))
            } else {
                None
            },
            desktop_rect: Some(rect(120.0, 180.0, 140.0, 70.0)),
            content_size: Some(p::Size {
                width: 140.0,
                height: 70.0,
            }),
            transform: Some(affine()),
            opacity: 1.0,
            stacking_order: 100,
            mapped: true,
            focused: transient.kind == TransientKind::Modal,
            scene_root: Some(reference(p::ObjectKind::Scene, id + 1)),
            semantic_root: Some(reference(p::ObjectKind::Semantic, id + 2)),
            input_region: if tooltip { vec![] } else { vec![region] },
            routing_token: format!("fixture-transient-{id}").into_bytes(),
            visual_coverage: p::VisualCoverage::Complete as i32,
            semantic_coverage: p::SemanticCoverage::Rich as i32,
            capture_consistency: p::CaptureConsistency::Coherent as i32,
            source_capture_tokens: vec![commit.to_be_bytes().to_vec()],
            ..Default::default()
        })));
        records.push(wrap(p::record::Record::Scene(scene(
            id + 1,
            1,
            region,
            [0.6, 0.3, 0.15, 1.0],
        ))));
        let mut n = node(
            id + 2,
            if tooltip {
                p::SemanticRole::Label
            } else {
                p::SemanticRole::Button
            },
            if tooltip { "Source tooltip" } else { "Dismiss" },
            self.modal.is_none() || transient.kind == TransientKind::Modal,
            projection,
            if tooltip { &[] } else { &["Activate"] },
        );
        n.visual_bindings = vec![reference(p::ObjectKind::Binding, id + 3)];
        records.push(wrap(p::record::Record::Semantic(n)));
        let mut b = binding(id + 3, id + 2, id + 1, region);
        b.surface = Some(reference(p::ObjectKind::Surface, id));
        records.push(wrap(p::record::Record::Binding(b)));
    }
}
fn action(name: &str) -> p::Action {
    p::Action {
        id: name.into(),
        schema_version: 1,
    }
}
fn node(
    id: u64,
    role: p::SemanticRole,
    label: &str,
    enabled: bool,
    projection: &Projection,
    actions: &[&str],
) -> p::SemanticNode {
    p::SemanticNode {
        id,
        version: 1,
        role: role as i32,
        label: Some(label.into()),
        states: vec![p::StateValue {
            key: p::StateKey::Disabled as i32,
            value: if enabled {
                p::TriState::False
            } else {
                p::TriState::True
            } as i32,
        }],
        actions: if projection.permissions.can_control {
            actions
                .iter()
                .filter(|s| projection.permissions.actions.contains(**s))
                .map(|s| action(s))
                .collect()
        } else {
            vec![]
        },
        sensitivity: p::Sensitivity::Public as i32,
        completeness: p::Completeness::Full as i32,
        ..Default::default()
    }
}
