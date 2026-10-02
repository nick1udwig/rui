//! Private, cooperating application state. Nothing from this module executes on
//! a client, and the source does not derive scene geometry from secret text.
use crate::corpus::Corpus;
use crate::{HostError, Projection, READ_SECRET_PERMISSION};
use rui_protocol_core::commands::{
    Arguments, Rejection, SemanticAuthority, SemanticCommand, TargetState,
};
use rui_protocol_core::wire::proto as p;
use std::collections::BTreeMap;

pub const SURFACE_ID: u64 = 10;
pub const SCENE_ROOT_ID: u64 = 20;
pub const COUNTER_SCENE_ID: u64 = 21;
pub const SECRET_SCENE_ID: u64 = 22;
pub const SEMANTIC_ROOT_ID: u64 = 30;
pub const COUNTER_ID: u64 = 31;
pub const SECRET_ID: u64 = 32;
pub const SECRET_TEXT_ID: u64 = 40;
pub const COUNTER_BINDING_ID: u64 = 41;
pub const SECRET_BINDING_ID: u64 = 42;
pub const SESSION_STATE_ID: u64 = 50;

pub(crate) struct ControlledApplication {
    counter: i64,
    secret: String,
    max_text_bytes: usize,
    counter_enabled: bool,
    pub commit: u64,
    pub counter_clock: u64,
    pub text_clock: u64,
    last_now: u64,
    pub corpus: Option<Corpus>,
    pub fonts: Option<crate::source_fonts::SourceFonts>,
}
impl ControlledApplication {
    pub fn new(secret: String, max_text_bytes: usize) -> Result<Self, HostError> {
        if max_text_bytes == 0 || max_text_bytes > 64 * 1024 || secret.len() > max_text_bytes {
            return Err(HostError::FixtureLimit);
        }
        Ok(Self {
            counter: 0,
            secret,
            max_text_bytes,
            counter_enabled: true,
            commit: 1,
            counter_clock: 1,
            text_clock: 1,
            last_now: 0,
            corpus: None,
            fonts: None,
        })
    }
    pub fn captions(&self) -> Vec<crate::source_fonts::Caption> {
        let mut captions = self.corpus.as_ref().map_or_else(Vec::new, |c| c.captions());
        captions.push(crate::source_fonts::Caption {
            slot: 0,
            scene: COUNTER_SCENE_ID,
            surface: SURFACE_ID,
            text: format!("Count {}", self.counter),
        });
        captions
    }
    pub fn refresh_fonts(&mut self) {
        let captions = self.captions();
        if let Some(fonts) = &mut self.fonts {
            fonts.refresh(&captions);
        }
    }
    pub fn source_resources(
        &self,
        projection: &Projection,
    ) -> &[crate::source_fonts::SourceOutline] {
        if projection.semantic_only
            || !projection
                .profiles
                .contains(crate::source_fonts::GLYPH_PROFILE)
        {
            return &[];
        }
        self.fonts.as_ref().map_or(&[], |f| f.resources())
    }
    pub fn counter(&self) -> i64 {
        self.counter
    }
    fn can_read_secret(projection: &Projection) -> bool {
        projection
            .permissions
            .actions
            .contains(READ_SECRET_PERMISSION)
    }
    pub fn authority(&self, projection: &Projection) -> SemanticAuthority {
        let target = |version,
                      enabled,
                      actions: BTreeMap<String, u32>,
                      text_version,
                      text_scalar_count,
                      focus_generation| TargetState {
            enabled,
            target_version: version,
            text_version,
            text_scalar_count,
            focus_generation,
            model_version: None,
            value_schema: None,
            actions,
        };
        let text_actions = if Self::can_read_secret(projection) {
            [("ReplaceText".into(), 1)].into_iter().collect()
        } else {
            BTreeMap::new()
        };
        let mut authority = SemanticAuthority {
            view_id: projection.view_id,
            current_view_revision: projection.revision.unwrap_or(0),
            allowed_actions: projection.permissions.actions.clone(),
            targets: [
                (
                    COUNTER_ID,
                    target(
                        self.counter_clock,
                        self.counter_enabled,
                        [("Activate".into(), 1)].into_iter().collect(),
                        None,
                        None,
                        None,
                    ),
                ),
                (
                    SECRET_ID,
                    target(
                        self.text_clock,
                        true,
                        text_actions,
                        Some(self.text_clock),
                        Some(self.secret.chars().count() as u64),
                        Some(1),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        };
        if let Some(corpus) = &self.corpus {
            corpus.extend_authority(&mut authority, projection);
            if let Some(private) = authority.targets.get_mut(&SECRET_ID) {
                private.focus_generation = Some(corpus.focus_generation);
                private.actions.insert("Focus".into(), 1);
            }
        }
        for (id, target) in &mut authority.targets {
            if let Some(stamp) = projection.records.get(id) {
                target.target_version = stamp.version;
            }
        }
        authority
    }
    pub fn validate(
        &self,
        projection: &Projection,
        command: &SemanticCommand,
    ) -> Result<(), Rejection> {
        self.authority(projection).validate(command)?;
        if let Some(corpus) = &self.corpus {
            if command.target_id != COUNTER_ID && command.target_id != SECRET_ID {
                return corpus.validate(command, self.max_text_bytes);
            }
            if command.action_id == "Focus" && command.target_id == SECRET_ID {
                return Ok(());
            }
        }
        if command.action_id == "ReplaceText" {
            let focus = self.corpus.as_ref().map_or(1, |c| c.focus_generation);
            if command.preconditions.focus_generation != Some(focus)
                || self.corpus.as_ref().is_some_and(|c| c.focus != SECRET_ID)
            {
                return Err(Rejection::StaleFocus);
            }
            let Arguments::ReplaceText { start, end, text } = &command.arguments else {
                return Err(Rejection::InvalidArguments);
            };
            let start = scalar_byte(&self.secret, *start).ok_or(Rejection::InvalidArguments)?;
            let end = scalar_byte(&self.secret, *end).ok_or(Rejection::InvalidArguments)?;
            let length = start
                .checked_add(text.len())
                .and_then(|n| n.checked_add(self.secret.len() - end));
            if length.is_none_or(|n| n > self.max_text_bytes) {
                return Err(Rejection::InvalidArguments);
            }
        }
        Ok(())
    }
    /// A real source commit: all checks and fallible counter arithmetic precede
    /// mutation. Unicode ranges are scalar indices, never UTF-8 byte offsets.
    pub fn apply(
        &mut self,
        projection: &Projection,
        command: &SemanticCommand,
        now: u64,
    ) -> Result<u64, Rejection> {
        self.validate(projection, command)?;
        let commit = self
            .commit
            .checked_add(1)
            .ok_or(Rejection::InvalidArguments)?;
        if let Some(corpus) = &mut self.corpus {
            if command.target_id != COUNTER_ID && command.target_id != SECRET_ID {
                corpus.apply(command, self.max_text_bytes)?;
                self.last_now = self.last_now.max(now);
                self.commit = commit;
                self.refresh_fonts();
                return Ok(commit);
            }
            if command.action_id == "Focus" && command.target_id == SECRET_ID {
                corpus.focus(SECRET_ID)?;
                self.commit = commit;
                self.last_now = self.last_now.max(now);
                self.refresh_fonts();
                return Ok(commit);
            }
        }
        match (&*command.action_id, &command.arguments) {
            ("Activate", Arguments::None) => {
                let counter = self
                    .counter
                    .checked_add(1)
                    .ok_or(Rejection::InvalidArguments)?;
                let clock = self
                    .counter_clock
                    .checked_add(1)
                    .ok_or(Rejection::InvalidArguments)?;
                self.counter = counter;
                self.counter_clock = clock;
            }
            ("ReplaceText", Arguments::ReplaceText { start, end, text }) => {
                let start = scalar_byte(&self.secret, *start).ok_or(Rejection::InvalidArguments)?;
                let end = scalar_byte(&self.secret, *end).ok_or(Rejection::InvalidArguments)?;
                let mut replacement =
                    String::with_capacity(start + text.len() + self.secret.len() - end);
                replacement.push_str(&self.secret[..start]);
                replacement.push_str(text);
                replacement.push_str(&self.secret[end..]);
                if replacement != self.secret {
                    let clock = self
                        .text_clock
                        .checked_add(1)
                        .ok_or(Rejection::InvalidArguments)?;
                    self.secret = replacement;
                    self.text_clock = clock;
                }
            }
            _ => return Err(Rejection::UnsupportedAction),
        }
        self.last_now = self.last_now.max(now);
        self.commit = commit;
        self.refresh_fonts();
        Ok(commit)
    }
    pub fn set_counter_enabled(&mut self, enabled: bool, now: u64) -> Result<bool, HostError> {
        if self.counter_enabled == enabled {
            return Ok(false);
        }
        let commit = self.commit.checked_add(1).ok_or(HostError::FixtureLimit)?;
        let clock = self
            .counter_clock
            .checked_add(1)
            .ok_or(HostError::FixtureLimit)?;
        self.counter_enabled = enabled;
        self.counter_clock = clock;
        self.commit = commit;
        self.last_now = self.last_now.max(now);
        Ok(true)
    }
    pub fn records(&self, projection: &Projection) -> Vec<p::Record> {
        let secret_visible = Self::can_read_secret(projection);
        let editable = projection.permissions.can_control
            && projection.permissions.actions.contains("ReplaceText")
            && secret_visible;
        let counter_active = projection.permissions.can_control
            && projection.permissions.actions.contains("Activate");
        let counter_rect = rect(8.0, 8.0, 72.0, 32.0);
        let secret_rect = rect(8.0, 48.0, 120.0, 32.0);
        let color = if !self.counter_enabled {
            [0.2, 0.2, 0.2, 1.0]
        } else if self.counter % 2 == 0 {
            [0.1, 0.4, 0.8, 1.0]
        } else {
            [0.2, 0.7, 0.2, 1.0]
        };
        let surface = p::Surface {
            id: SURFACE_ID,
            version: self.commit,
            role: p::SurfaceRole::Toplevel as i32,
            desktop_rect: Some(rect(0.0, 0.0, 160.0, 100.0)),
            content_size: Some(p::Size {
                width: 160.0,
                height: 100.0,
            }),
            transform: Some(affine()),
            opacity: 1.0,
            mapped: true,
            focused: true,
            scene_root: Some(reference(p::ObjectKind::Scene, SCENE_ROOT_ID)),
            semantic_root: Some(reference(p::ObjectKind::Semantic, SEMANTIC_ROOT_ID)),
            input_region: vec![counter_rect, secret_rect],
            routing_token: b"fixture-route-1".to_vec(),
            visual_coverage: p::VisualCoverage::Complete as i32,
            semantic_coverage: p::SemanticCoverage::Rich as i32,
            capture_consistency: p::CaptureConsistency::Coherent as i32,
            source_capture_tokens: vec![self.commit.to_be_bytes().to_vec()],
            ..Default::default()
        };
        let root_scene = p::SceneNode {
            id: SCENE_ROOT_ID,
            version: 1,
            ordered_children: vec![
                reference(p::ObjectKind::Scene, COUNTER_SCENE_ID),
                reference(p::ObjectKind::Scene, SECRET_SCENE_ID),
            ],
            transform: Some(affine()),
            opacity: 1.0,
            primitive: Some(p::scene_node::Primitive::Group(p::Group {})),
            ..Default::default()
        };
        let counter_scene = scene(COUNTER_SCENE_ID, self.counter_clock, counter_rect, color);
        // A constant public vector rectangle: no glyphs, secret length, text
        // geometry, masked character count, or raster fallback are transmitted.
        let secret_scene = scene(SECRET_SCENE_ID, 1, secret_rect, [0.1, 0.1, 0.1, 1.0]);
        let root = p::SemanticNode {
            id: SEMANTIC_ROOT_ID,
            version: 1,
            role: p::SemanticRole::Root as i32,
            label: Some("Controlled fixture".into()),
            ordered_children: vec![
                reference(p::ObjectKind::Semantic, COUNTER_ID),
                reference(p::ObjectKind::Semantic, SECRET_ID),
            ],
            sensitivity: p::Sensitivity::Public as i32,
            completeness: p::Completeness::Full as i32,
            ..Default::default()
        };
        let counter = p::SemanticNode {
            id: COUNTER_ID,
            version: self.counter_clock,
            role: p::SemanticRole::Button as i32,
            label: Some("Increment".into()),
            typed_value: Some(p::TypedValue {
                value: Some(p::typed_value::Value::Integer(self.counter)),
            }),
            states: vec![p::StateValue {
                key: p::StateKey::Disabled as i32,
                value: if self.counter_enabled {
                    p::TriState::False
                } else {
                    p::TriState::True
                } as i32,
            }],
            actions: if counter_active {
                vec![p::Action {
                    id: "Activate".into(),
                    schema_version: 1,
                }]
            } else {
                vec![]
            },
            visual_bindings: vec![reference(p::ObjectKind::Binding, COUNTER_BINDING_ID)],
            sensitivity: p::Sensitivity::Public as i32,
            completeness: p::Completeness::Full as i32,
            ..Default::default()
        };
        let secret = p::SemanticNode {
            id: SECRET_ID,
            version: if secret_visible { self.text_clock } else { 1 },
            role: p::SemanticRole::TextField as i32,
            label: if secret_visible {
                Some("Private field".into())
            } else {
                None
            },
            text_model: Some(reference(p::ObjectKind::Text, SECRET_TEXT_ID)),
            states: vec![p::StateValue {
                key: p::StateKey::ReadOnly as i32,
                value: if editable {
                    p::TriState::False
                } else {
                    p::TriState::True
                } as i32,
            }],
            actions: if editable {
                vec![p::Action {
                    id: "ReplaceText".into(),
                    schema_version: 1,
                }]
            } else {
                vec![]
            },
            visual_bindings: vec![reference(p::ObjectKind::Binding, SECRET_BINDING_ID)],
            sensitivity: if secret_visible {
                p::Sensitivity::Sensitive
            } else {
                p::Sensitivity::Redacted
            } as i32,
            completeness: p::Completeness::Full as i32,
            ..Default::default()
        };
        let text = p::TextModel {
            id: SECRET_TEXT_ID,
            version: if secret_visible { self.text_clock } else { 1 },
            text_version: if secret_visible { self.text_clock } else { 1 },
            status: if secret_visible {
                p::TextStatus::Available
            } else {
                p::TextStatus::TextRedacted
            } as i32,
            content: if secret_visible {
                Some(self.secret.clone())
            } else {
                None
            },
            selection: None,
            editable,
            selectable: secret_visible,
        };
        let counter_binding = binding(
            COUNTER_BINDING_ID,
            COUNTER_ID,
            COUNTER_SCENE_ID,
            counter_rect,
        );
        let secret_binding = binding(SECRET_BINDING_ID, SECRET_ID, SECRET_SCENE_ID, secret_rect);
        let session = p::SessionState {
            id: SESSION_STATE_ID,
            version: 1,
            canvas: Some(rect(0.0, 0.0, 160.0, 100.0)),
            focus: Some(reference(p::ObjectKind::Semantic, SECRET_ID)),
            focus_surface: Some(reference(p::ObjectKind::Surface, SURFACE_ID)),
            focus_generation: 1,
            ..Default::default()
        };
        let mut records = vec![
            wrap(p::record::Record::Surface(surface)),
            wrap(p::record::Record::Scene(root_scene)),
            wrap(p::record::Record::Scene(counter_scene)),
            wrap(p::record::Record::Scene(secret_scene)),
            wrap(p::record::Record::Semantic(root)),
            wrap(p::record::Record::Semantic(counter)),
            wrap(p::record::Record::Semantic(secret)),
            wrap(p::record::Record::Text(text)),
            wrap(p::record::Record::Binding(counter_binding)),
            wrap(p::record::Record::Binding(secret_binding)),
            wrap(p::record::Record::Session(session)),
        ];
        if let Some(corpus) = &self.corpus {
            corpus.extend_records(&mut records, projection, self.commit);
        }
        if !projection.semantic_only {
            if let Some(fonts) = &self.fonts {
                fonts.project(
                    &mut records,
                    &self.captions(),
                    projection
                        .profiles
                        .contains(crate::source_fonts::GLYPH_PROFILE),
                );
            }
        }
        records
    }
    pub fn decoded_projection_bytes(&self, projection: &Projection) -> usize {
        // Conservative allowance for this fixed eleven-record fixture. The core
        // receiver independently accounts its actual decoded state and staging.
        32 * 1024
            + if self.fonts.is_some()
                && !projection.semantic_only
                && projection
                    .profiles
                    .contains(crate::source_fonts::GLYPH_PROFILE)
            {
                3 * 1024 * 1024
            } else {
                0
            }
            + if self.corpus.is_some() {
                256 * 1024 + self.max_text_bytes * 4
            } else {
                0
            }
            + if Self::can_read_secret(projection) {
                self.max_text_bytes * 4
            } else {
                0
            }
    }
}
pub(crate) fn scalar_byte(text: &str, scalar: u64) -> Option<usize> {
    let scalar = usize::try_from(scalar).ok()?;
    if scalar == text.chars().count() {
        Some(text.len())
    } else {
        text.char_indices().nth(scalar).map(|(offset, _)| offset)
    }
}
pub(crate) fn rect(x: f64, y: f64, width: f64, height: f64) -> p::Rect {
    p::Rect {
        x,
        y,
        width,
        height,
    }
}
pub(crate) fn affine() -> p::Affine {
    p::Affine {
        xx: 1.0,
        yy: 1.0,
        ..Default::default()
    }
}
pub(crate) fn reference(kind: p::ObjectKind, id: u64) -> p::ObjectRef {
    p::ObjectRef {
        kind: kind as i32,
        id,
    }
}
pub(crate) fn wrap(record: p::record::Record) -> p::Record {
    p::Record {
        record: Some(record),
    }
}
pub(crate) fn scene(id: u64, version: u64, rect: p::Rect, color: [f64; 4]) -> p::SceneNode {
    p::SceneNode {
        id,
        version,
        transform: Some(affine()),
        opacity: 1.0,
        primitive: Some(p::scene_node::Primitive::SolidRect(p::SolidRect {
            rect: Some(rect),
            color: Some(p::Color {
                red: color[0],
                green: color[1],
                blue: color[2],
                alpha: color[3],
            }),
        })),
        ..Default::default()
    }
}
pub(crate) fn binding(id: u64, semantic: u64, scene: u64, region: p::Rect) -> p::Binding {
    p::Binding {
        id,
        version: 1,
        surface: Some(reference(p::ObjectKind::Surface, SURFACE_ID)),
        semantic_nodes: vec![reference(p::ObjectKind::Semantic, semantic)],
        scene_nodes: vec![reference(p::ObjectKind::Scene, scene)],
        interaction_region: Some(region),
    }
}
