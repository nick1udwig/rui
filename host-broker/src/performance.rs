//! Fixed, resource-bearing 10,000-record source for reproducible load runs.
//! Every scene leaf is visible; semantic leaves describe those source cells.
use crate::{fixture::*, HostError, SourceFontProvider, SourceGlyph, SourceOutline};
use rui_protocol_core::wire::proto as p;

pub const PERFORMANCE_RECORDS: usize = 10_000;
pub const PERFORMANCE_WIDTH: f64 = 1000.0;
pub const PERFORMANCE_HEIGHT: f64 = 600.0;
pub const PERFORMANCE_GLYPHS: usize = 2500;
const SCENE_BASE: u64 = 10_000;
const SEMANTIC_BASE: u64 = 20_000;

pub(crate) struct PerformanceSource {
    glyph: SourceGlyph,
    resources: Vec<SourceOutline>,
    changed: [bool; 19],
    dim: bool,
}
impl PerformanceSource {
    pub fn new(provider: &dyn SourceFontProvider) -> Result<Self, HostError> {
        let shaped = provider
            .shape("O", 8.0)
            .map_err(|_| HostError::FixtureLimit)?;
        if shaped.glyphs.len() != 1 || shaped.resources.len() != 1 {
            return Err(HostError::FixtureLimit);
        }
        let glyph = shaped.glyphs.into_iter().next().unwrap();
        if glyph.resource != shaped.resources[0].offer().digest
            || glyph.size != 8.0
            || glyph
                .position
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 32.0)
            || shaped.resources[0].bytes().len() > 64 * 1024
        {
            return Err(HostError::FixtureLimit);
        }
        Ok(Self {
            glyph,
            resources: shaped.resources,
            changed: [false; 19],
            dim: false,
        })
    }
    pub fn resources(&self) -> &[SourceOutline] {
        &self.resources
    }
    pub fn advance(&mut self, changed_records: usize) -> Result<(), HostError> {
        if !matches!(changed_records, 1 | 20) {
            return Err(HostError::FixtureLimit);
        }
        self.dim = !self.dim;
        if changed_records == 20 {
            for value in &mut self.changed {
                *value = !*value;
            }
        }
        Ok(())
    }
    pub fn records(&self, commit: u64) -> Vec<p::Record> {
        let mut records = Vec::with_capacity(PERFORMANCE_RECORDS);
        records.push(wrap(p::record::Record::Session(p::SessionState {
            id: SESSION_STATE_ID,
            version: 1,
            canvas: Some(rect(0.0, 0.0, PERFORMANCE_WIDTH, PERFORMANCE_HEIGHT)),
            ..Default::default()
        })));
        records.push(wrap(p::record::Record::Surface(p::Surface {
            id: SURFACE_ID,
            version: 1,
            role: p::SurfaceRole::Toplevel as i32,
            desktop_rect: Some(rect(0.0, 0.0, PERFORMANCE_WIDTH, PERFORMANCE_HEIGHT)),
            content_size: Some(p::Size {
                width: PERFORMANCE_WIDTH,
                height: PERFORMANCE_HEIGHT,
            }),
            transform: Some(affine()),
            opacity: if self.dim { 0.9 } else { 1.0 },
            mapped: true,
            scene_root: Some(reference(p::ObjectKind::Scene, SCENE_ROOT_ID)),
            semantic_root: Some(reference(p::ObjectKind::Semantic, SEMANTIC_ROOT_ID)),
            visual_coverage: p::VisualCoverage::Complete as i32,
            semantic_coverage: p::SemanticCoverage::Rich as i32,
            capture_consistency: p::CaptureConsistency::Coherent as i32,
            source_capture_tokens: vec![commit.to_be_bytes().to_vec()],
            ..Default::default()
        })));
        let mut next = 0;
        self.region(&mut records, &mut next, [0, 0, 50, 50]);
        assert_eq!(next, 2 * PERFORMANCE_GLYPHS - 1);
        records
    }
    fn region(
        &self,
        records: &mut Vec<p::Record>,
        next: &mut usize,
        region: [usize; 4],
    ) -> (u64, u64) {
        let [x, y, width, height] = region;
        let index = *next;
        *next += 1;
        let scene = if index == 0 {
            SCENE_ROOT_ID
        } else {
            SCENE_BASE + index as u64
        };
        let semantic = if index == 0 {
            SEMANTIC_ROOT_ID
        } else {
            SEMANTIC_BASE + index as u64
        };
        let mut scene_children = Vec::new();
        let mut semantic_children = Vec::new();
        let leaf = width == 1 && height == 1;
        if !leaf {
            let regions = if width >= height {
                let half = width / 2;
                [[x, y, half, height], [x + half, y, width - half, height]]
            } else {
                let half = height / 2;
                [[x, y, width, half], [x, y + half, width, height - half]]
            };
            for region in regions {
                let (scene, semantic) = self.region(records, next, region);
                scene_children.push(reference(p::ObjectKind::Scene, scene));
                semantic_children.push(reference(p::ObjectKind::Semantic, semantic));
            }
        }
        let primitive = if leaf {
            p::scene_node::Primitive::GlyphRun(p::GlyphRun {
                font: Some(p::glyph_run::Font::OutlineResource(
                    self.glyph.resource.to_vec(),
                )),
                size: self.glyph.size,
                glyph_ids: vec![0],
                positions: vec![p::Point {
                    x: x as f64 * 20.0 + self.glyph.position[0],
                    y: y as f64 * 12.0 + self.glyph.position[1],
                }],
                scalar_clusters: vec![0],
                color: Some(p::Color {
                    red: if self.changed.get(y * 50 + x) == Some(&true) {
                        0.8
                    } else {
                        0.2
                    },
                    green: 0.6,
                    blue: 0.9,
                    alpha: 1.0,
                }),
            })
        } else {
            p::scene_node::Primitive::Group(p::Group {})
        };
        records.push(wrap(p::record::Record::Scene(p::SceneNode {
            id: scene,
            version: 1,
            transform: Some(affine()),
            opacity: 1.0,
            clip: Some(rect(
                x as f64 * 20.0,
                y as f64 * 12.0,
                width as f64 * 20.0,
                height as f64 * 12.0,
            )),
            ordered_children: scene_children,
            resource_refs: if leaf {
                vec![self.glyph.resource.to_vec()]
            } else {
                vec![]
            },
            primitive: Some(primitive),
        })));
        records.push(wrap(p::record::Record::Semantic(p::SemanticNode {
            browser_input_type: None,
            id: semantic,
            version: 1,
            role: if index == 0 {
                p::SemanticRole::Root
            } else if leaf {
                p::SemanticRole::Label
            } else {
                p::SemanticRole::GroupRole
            } as i32,
            label: Some(if leaf {
                format!("Cell {x},{y}: O")
            } else {
                format!("Region {x},{y} {width}x{height}")
            }),
            ordered_children: semantic_children,
            sensitivity: p::Sensitivity::Public as i32,
            completeness: p::Completeness::Full as i32,
            ..Default::default()
        })));
        (scene, semantic)
    }
}
