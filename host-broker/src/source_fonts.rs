//! Trusted source-native shaping. Only explicitly public captions enter this
//! boundary; private field content and lengths never do.
use crate::fixture::{affine, reference, wrap};
use rui_protocol_core::resources::{
    resource_digest, validate_vector_bytes, ResourceDigest, ResourceKind, ResourceLimits,
    ResourceOffer, ResourceProvenance,
};
use rui_protocol_core::wire::proto as p;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use thiserror::Error;

pub const GLYPH_PROFILE: &str = "glyph.outline-single.v1";
const CAPTIONS: usize = 16;
const GLYPHS_PER_CAPTION: usize = 128;
const RESOURCE_COUNT: usize = 512;
const SOURCE_RESOURCE_BYTES: usize = 8 * 1024 * 1024;
const SLOT_ID: u64 = 100_000;
const BACKGROUND_ID: u64 = 90_000;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("source font input, output or environment is unsupported")]
pub struct SourceFontError;
/// Host-owned shaping capability. Implementations must bound their own work
/// before allocating; the broker independently verifies returned outlines,
/// descriptor hashes, numeric ranges, count and aggregate byte ceilings. A
/// failure changes visual coverage to Partial and does not reject text edits.
pub trait SourceFontProvider: Send + Sync {
    fn shape(&self, public_text: &str, size: f64) -> Result<SourceShapedText, SourceFontError>;
}
#[derive(Clone, Debug)]
pub struct SourceGlyph {
    pub resource: ResourceDigest,
    pub size: f64,
    pub position: [f64; 2],
}
/// Exact source-shaped sequence, including overlap order. Resources contain no
/// font files or raster data. Debug exposes only counts, never source content.
pub struct SourceShapedText {
    pub glyphs: Vec<SourceGlyph>,
    pub resources: Vec<SourceOutline>,
}
impl std::fmt::Debug for SourceShapedText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceShapedText")
            .field("glyphs", &self.glyphs.len())
            .field("resources", &self.resources.len())
            .finish()
    }
}
#[derive(Clone)]
pub struct SourceOutline {
    offer: ResourceOffer,
    body: Arc<[u8]>,
}
impl SourceOutline {
    pub fn new(offer: ResourceOffer, body: Vec<u8>) -> Result<Self, SourceFontError> {
        if offer.kind != ResourceKind::GlyphOutline
            || offer.version != 1
            || offer.provenance != ResourceProvenance::FontOutline
            || offer.total_bytes != body.len() as u64
            || offer.digest != resource_digest(offer.kind, offer.version, offer.provenance, &body)
        {
            return Err(SourceFontError);
        }
        validate_vector_bytes(
            &body,
            &ResourceLimits {
                max_resource_bytes: 4 * 1024 * 1024,
                max_vector_commands: 4096,
                ..Default::default()
            },
        )
        .map_err(|_| SourceFontError)?;
        Ok(Self {
            offer,
            body: body.into(),
        })
    }
    pub fn offer(&self) -> &ResourceOffer {
        &self.offer
    }
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }
}
impl std::fmt::Debug for SourceOutline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceOutline")
            .field("bytes", &self.body.len())
            .finish()
    }
}
/// Explicit local producer IPC using the repository's pinned Pango/Cairo/font
/// helper. Paths are supplied by the trusted host, never by a client. No shell
/// runs; stdin, stdout, time and decoded native resources have fixed ceilings.
pub struct NativeSourceFontProvider {
    python: PathBuf,
    helper: PathBuf,
    deadline: Duration,
}
impl NativeSourceFontProvider {
    pub fn new(python: PathBuf, helper: PathBuf) -> Self {
        Self {
            python,
            helper,
            deadline: Duration::from_secs(3),
        }
    }
}
impl SourceFontProvider for NativeSourceFontProvider {
    fn shape(&self, public_text: &str, size: f64) -> Result<SourceShapedText, SourceFontError> {
        if public_text.len() > 2048 || !size.is_finite() || !(1.0..=256.0).contains(&size) {
            return Err(SourceFontError);
        }
        let input = serde_json::to_vec(&serde_json::json!({"text":public_text,"size":size}))
            .map_err(|_| SourceFontError)?;
        if input.len() > 4096 {
            return Err(SourceFontError);
        }
        let mut child = Command::new(&self.python)
            .arg(&self.helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| SourceFontError)?;
        if child
            .stdin
            .take()
            .ok_or(SourceFontError)?
            .write_all(&input)
            .is_err()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SourceFontError);
        }
        let stdout = child.stdout.take().ok_or(SourceFontError)?;
        let (tx, rx) = mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
                .map_err(|_| SourceFontError);
            let _ = tx.send(result);
        });
        let until = Instant::now() + self.deadline;
        let bytes = match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(Ok(bytes)) if bytes.len() <= 8 * 1024 * 1024 => bytes,
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(SourceFontError);
            }
        };
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(1)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(SourceFontError);
                }
            }
        };
        let _ = reader.join();
        if !status.success() {
            return Err(SourceFontError);
        }
        // This trusted producer response has an 8MiB whole-output ceiling before
        // serde. Every list/string therefore has a bounded decoded allocation;
        // source IPC scratch is separately capped (not retained as view state).
        let output: NativeOutput = serde_json::from_slice(&bytes).map_err(|_| SourceFontError)?;
        if output.profile != "source.pango1582.cairo1184.liberation-sans.v1"
            || output.font_sha256
                != "baccc64becc3eb7d104b7c84d99f5314a0a1f896e2b3ea6c2f22fc08d2003bee"
            || output.glyphs.0.len() > GLYPHS_PER_CAPTION
            || output.resources.0.len() > RESOURCE_COUNT
        {
            return Err(SourceFontError);
        }
        let mut resources = Vec::new();
        let mut total = 0usize;
        for r in output.resources.0 {
            let body = hex(&r.body_hex)?;
            total = total.checked_add(body.len()).ok_or(SourceFontError)?;
            if total > 4 * 1024 * 1024 {
                return Err(SourceFontError);
            }
            let digest = digest(&r.digest)?;
            resources.push(SourceOutline::new(
                ResourceOffer {
                    kind: ResourceKind::try_from(r.kind).map_err(|_| SourceFontError)?,
                    version: r.version,
                    total_bytes: r.total_bytes,
                    provenance: ResourceProvenance::try_from(r.provenance)
                        .map_err(|_| SourceFontError)?,
                    digest,
                },
                body,
            )?);
        }
        let glyphs = output
            .glyphs
            .0
            .into_iter()
            .map(|g| {
                Ok(SourceGlyph {
                    resource: digest(&g.resource)?,
                    size: g.size,
                    position: g.position,
                })
            })
            .collect::<Result<_, SourceFontError>>()?;
        Ok(SourceShapedText { glyphs, resources })
    }
}
#[derive(Deserialize)]
struct NativeOutput {
    profile: String,
    font_sha256: String,
    glyphs: BoundedVec<NativeGlyph, 128>,
    resources: BoundedVec<NativeResource, 512>,
}
#[derive(Deserialize)]
struct NativeGlyph {
    resource: String,
    size: f64,
    position: [f64; 2],
}
#[derive(Deserialize)]
struct NativeResource {
    digest: String,
    kind: u32,
    version: u32,
    provenance: u32,
    total_bytes: u64,
    body_hex: String,
}
struct BoundedVec<T, const N: usize>(Vec<T>);
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for BoundedVec<T, N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor<T, const N: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Visitor<T, N> {
            type Value = BoundedVec<T, N>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "at most {N} entries")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut result = Vec::new();
                while let Some(value) = seq.next_element()? {
                    if result.len() >= N {
                        return Err(serde::de::Error::custom("source list ceiling"));
                    }
                    result.push(value);
                }
                Ok(BoundedVec(result))
            }
        }
        deserializer.deserialize_seq(Visitor::<T, N>(std::marker::PhantomData))
    }
}
fn hex(s: &str) -> Result<Vec<u8>, SourceFontError> {
    if !s.len().is_multiple_of(2) {
        return Err(SourceFontError);
    }
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |v: u8| match v {
                b'0'..=b'9' => Ok(v - b'0'),
                b'a'..=b'f' => Ok(v - b'a' + 10),
                _ => Err(SourceFontError),
            };
            Ok(digit(pair[0])? * 16 + digit(pair[1])?)
        })
        .collect()
}
fn digest(s: &str) -> Result<ResourceDigest, SourceFontError> {
    if s.len() != 64 {
        return Err(SourceFontError);
    }
    hex(s)?.try_into().map_err(|_| SourceFontError)
}

pub(crate) struct Caption {
    pub slot: usize,
    pub scene: u64,
    pub surface: u64,
    pub text: String,
}
struct CachedCaption {
    text: String,
    shape: Option<SourceShapedText>,
}
pub(crate) struct SourceFonts {
    provider: Arc<dyn SourceFontProvider>,
    captions: BTreeMap<usize, CachedCaption>,
    resources: Vec<SourceOutline>,
    complete: bool,
}
impl SourceFonts {
    pub fn new(provider: Arc<dyn SourceFontProvider>) -> Self {
        Self {
            provider,
            captions: BTreeMap::new(),
            resources: vec![],
            complete: false,
        }
    }
    pub fn refresh(&mut self, captions: &[Caption]) {
        let mut complete = true;
        let mut pool: BTreeMap<ResourceDigest, SourceOutline> = BTreeMap::new();
        let mut total = 0usize;
        for caption in captions {
            if caption.slot >= CAPTIONS {
                complete = false;
                continue;
            }
            if self
                .captions
                .get(&caption.slot)
                .is_none_or(|c| c.text != caption.text)
            {
                let shape = self
                    .provider
                    .shape(&caption.text, 14.0)
                    .ok()
                    .filter(valid_shape);
                self.captions.insert(
                    caption.slot,
                    CachedCaption {
                        text: caption.text.clone(),
                        shape,
                    },
                );
            }
            let cache = self.captions.get_mut(&caption.slot).unwrap();
            let Some(shape) = cache.shape.as_mut() else {
                complete = false;
                continue;
            };
            let mut fits = true;
            for resource in &mut shape.resources {
                if let Some(shared) = pool.get(&resource.offer.digest) {
                    *resource = shared.clone();
                } else if total + resource.body.len() <= SOURCE_RESOURCE_BYTES
                    && pool.len() < RESOURCE_COUNT
                {
                    total += resource.body.len();
                    pool.insert(resource.offer.digest, resource.clone());
                } else {
                    fits = false;
                }
            }
            if !fits {
                cache.shape = None;
                complete = false;
            }
        }
        self.captions
            .retain(|slot, _| captions.iter().any(|c| *slot == c.slot));
        // Pool and caches share each canonical body. At most the old 8MiB and
        // next 8MiB source closures coexist during serial refresh.
        self.resources = pool.into_values().collect();
        self.complete = complete;
    }
    pub fn complete(&self) -> bool {
        self.complete
    }
    pub fn resources(&self) -> &[SourceOutline] {
        &self.resources
    }
    pub fn project(
        &self,
        records: &mut Vec<p::Record>,
        captions: &[Caption],
        glyph_selected: bool,
    ) {
        if !glyph_selected {
            for record in records.iter_mut().filter_map(|r| r.record.as_mut()) {
                if let p::record::Record::Surface(s) = record {
                    if captions.iter().any(|c| c.surface == s.id) {
                        s.visual_coverage = p::VisualCoverage::Partial as i32;
                        s.unsupported_features = vec!["source-glyph-profile-not-selected".into()];
                    }
                }
            }
            return;
        }
        // All slots exist from the first projection and persist while dormant.
        // Source sequence is preserved exactly; outlines are never grouped by
        // digest because that could change overlapping glyph painting order.
        for slot in 0..CAPTIONS * GLYPHS_PER_CAPTION {
            records.push(wrap(p::record::Record::Scene(group(SLOT_ID + slot as u64))));
        }
        for slot in 0..CAPTIONS {
            records.push(wrap(p::record::Record::Scene(group(
                BACKGROUND_ID + slot as u64,
            ))));
        }
        let mut active = BTreeSet::new();
        for caption in captions {
            let Some(cache) = self.captions.get(&caption.slot) else {
                continue;
            };
            let Some(shape) = &cache.shape else {
                continue;
            };
            if !shape.resources.iter().all(|r| {
                self.resources
                    .iter()
                    .any(|v| v.offer.digest == r.offer.digest)
            }) {
                continue;
            }
            let index = records.iter().position(
                |r| matches!(&r.record, Some(p::record::Record::Scene(s)) if s.id == caption.scene),
            );
            let Some(index) = index else {
                continue;
            };
            let Some(p::record::Record::Scene(parent)) = records[index].record.as_mut() else {
                continue;
            };
            let Some(p::scene_node::Primitive::SolidRect(background)) = parent.primitive.take()
            else {
                continue;
            };
            let region = background.rect.unwrap_or_default();
            let mut bg = group(BACKGROUND_ID + caption.slot as u64);
            bg.primitive = Some(p::scene_node::Primitive::SolidRect(background));
            parent.primitive = Some(p::scene_node::Primitive::Group(p::Group {}));
            parent.ordered_children = vec![reference(p::ObjectKind::Scene, bg.id)];
            active.insert(bg.id);
            let mut nodes = Vec::new();
            for (i, glyph) in shape.glyphs.iter().enumerate() {
                let id = SLOT_ID + (caption.slot * GLYPHS_PER_CAPTION + i) as u64;
                parent
                    .ordered_children
                    .push(reference(p::ObjectKind::Scene, id));
                active.insert(id);
                let mut node = group(id);
                node.clip = Some(region);
                node.resource_refs = vec![glyph.resource.to_vec()];
                node.primitive = Some(p::scene_node::Primitive::GlyphRun(p::GlyphRun {
                    font: Some(p::glyph_run::Font::OutlineResource(glyph.resource.to_vec())),
                    size: glyph.size,
                    glyph_ids: vec![0],
                    positions: vec![p::Point {
                        x: region.x + 4.0 + glyph.position[0],
                        y: region.y + 4.0 + glyph.position[1],
                    }],
                    scalar_clusters: vec![],
                    color: Some(p::Color {
                        red: 0.95,
                        green: 0.95,
                        blue: 0.95,
                        alpha: 1.0,
                    }),
                }));
                nodes.push(node);
            }
            for node in nodes {
                let id = node.id;
                let target = records
                    .iter_mut()
                    .find(|r| matches!(&r.record, Some(p::record::Record::Scene(s)) if s.id == id))
                    .unwrap();
                *target = wrap(p::record::Record::Scene(node));
            }
            let bg_id = bg.id;
            let target = records
                .iter_mut()
                .find(|r| matches!(&r.record, Some(p::record::Record::Scene(s)) if s.id == bg_id))
                .unwrap();
            *target = wrap(p::record::Record::Scene(bg));
        }
        let mut dormant = group(89_999);
        dormant.ordered_children = (0..CAPTIONS * GLYPHS_PER_CAPTION)
            .map(|slot| SLOT_ID + slot as u64)
            .chain((0..CAPTIONS).map(|slot| BACKGROUND_ID + slot as u64))
            .filter(|id| !active.contains(id))
            .map(|id| reference(p::ObjectKind::Scene, id))
            .collect();
        let main = records
            .iter_mut()
            .find_map(|r| match r.record.as_mut() {
                Some(p::record::Record::Scene(s)) if s.id == crate::fixture::SCENE_ROOT_ID => {
                    Some(s)
                }
                _ => None,
            })
            .unwrap();
        main.ordered_children
            .push(reference(p::ObjectKind::Scene, dormant.id));
        records.push(wrap(p::record::Record::Scene(dormant)));
        for record in records.iter_mut().filter_map(|r| r.record.as_mut()) {
            if let p::record::Record::Surface(s) = record {
                if captions.iter().any(|c| c.surface == s.id) {
                    let complete = captions.iter().filter(|c| c.surface == s.id).all(|c| {
                        self.captions
                            .get(&c.slot)
                            .is_some_and(|c| c.shape.is_some())
                    }) && self.complete;
                    s.visual_coverage = if complete {
                        p::VisualCoverage::Complete
                    } else {
                        p::VisualCoverage::Partial
                    } as i32;
                    s.unsupported_features = if complete {
                        vec![]
                    } else {
                        vec!["source-glyph-coverage-gap".into()]
                    };
                }
            }
        }
    }
}
fn group(id: u64) -> p::SceneNode {
    p::SceneNode {
        id,
        version: 1,
        transform: Some(affine()),
        opacity: 1.0,
        primitive: Some(p::scene_node::Primitive::Group(p::Group {})),
        ..Default::default()
    }
}
fn valid_shape(shape: &SourceShapedText) -> bool {
    let mut resources = BTreeMap::new();
    let mut bytes = 0usize;
    if shape.glyphs.len() > GLYPHS_PER_CAPTION || shape.resources.len() > RESOURCE_COUNT {
        return false;
    }
    for resource in &shape.resources {
        bytes += resource.body.len();
        if bytes > 4 * 1024 * 1024 || resources.insert(resource.offer.digest, ()).is_some() {
            return false;
        }
    }
    shape.glyphs.iter().all(|g| {
        (1.0..=256.0).contains(&g.size)
            && g.position
                .iter()
                .all(|x| x.is_finite() && x.abs() <= 1_000_000.0)
            && resources.contains_key(&g.resource)
    })
}
