//! Local deterministic artifact tool. Resource descriptors are trusted fixture
//! inputs; this executable is not an authorization or network boundary.
use rui_client_desktop::*;
use rui_protocol_core::model::{ApplyOutcome, Limits, Replica};
use rui_protocol_core::resources::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

struct OwnedBytes {
    bytes: Vec<u8>,
    _memory: MemoryReservation,
}
fn read(
    path: &Path,
    maximum: usize,
    budget: &MemoryBudget,
) -> Result<OwnedBytes, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let length = usize::try_from(file.metadata()?.len())?;
    if length > maximum {
        return Err("fixture file exceeds limit".into());
    }
    let memory = budget.reserve(length.checked_add(256).ok_or("size overflow")?)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length)?;
    bytes.resize(length, 0);
    file.read_exact(&mut bytes)?;
    let mut extra = [0];
    if file.read(&mut extra)? != 0 {
        return Err("fixture file grew while reading".into());
    }
    Ok(OwnedBytes {
        bytes,
        _memory: memory,
    })
}
struct LocalResources {
    entries: BTreeMap<[u8; 32], (ResourceOffer, OwnedBytes, MemoryReservation)>,
}
impl ResourceProvider for LocalResources {
    fn get(&self, id: &[u8; 32]) -> Option<ResourceView<'_>> {
        self.entries.get(id).map(|(offer, body, _)| ResourceView {
            offer,
            bytes: &body.bytes,
        })
    }
}
fn digest(text: &str) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("digest must be 64 lowercase hex digits".into());
    }
    let mut result = [0; 32];
    for (i, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)?;
    }
    Ok(result)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if !(args.len() == 7 || args.len() == 8) {
        return Err("usage: render-snapshot SNAPSHOT.pb DESCRIPTORS.tsv RESOURCE_DIR OUTPUT.pam WIDTH HEIGHT [SCALE]".into());
    }
    let budget = MemoryBudget::new(128 << 20);
    let limits = Limits::default();
    let snapshot = read(Path::new(&args[1]), limits.max_snapshot_bytes, &budget)?;
    let descriptors = read(Path::new(&args[2]), 1 << 20, &budget)?;
    let mut resources = LocalResources {
        entries: BTreeMap::new(),
    };
    for line in std::str::from_utf8(&descriptors.bytes)?
        .lines()
        .filter(|line| !line.is_empty())
    {
        if resources.entries.len() >= 4096 {
            return Err("too many resource descriptors".into());
        }
        let mut fields = line.split_ascii_whitespace();
        let text = fields.next().ok_or("missing digest")?;
        let id = digest(text)?;
        let kind = ResourceKind::try_from(fields.next().ok_or("missing kind")?.parse::<u32>()?)?;
        let version = fields.next().ok_or("missing version")?.parse::<u32>()?;
        let provenance = ResourceProvenance::try_from(
            fields.next().ok_or("missing provenance")?.parse::<u32>()?,
        )?;
        if fields.next().is_some() || version != 1 || resources.entries.contains_key(&id) {
            return Err("invalid descriptor".into());
        }
        let entry_memory = budget.reserve(4096)?;
        let body = read(
            &Path::new(&args[3]).join(format!("{text}.rvp")),
            16 << 20,
            &budget,
        )?;
        if resource_digest(kind, version, provenance, &body.bytes) != id {
            return Err("resource integrity mismatch".into());
        }
        validate_vector_bytes(&body.bytes, &ResourceLimits::default())?;
        if !matches!(
            (kind, provenance),
            (ResourceKind::VectorPath, ResourceProvenance::AuthoredVector)
                | (ResourceKind::GlyphOutline, ResourceProvenance::FontOutline)
        ) {
            return Err("unsupported provenance".into());
        }
        let offer = ResourceOffer {
            kind,
            version,
            provenance,
            total_bytes: body.bytes.len() as u64,
            digest: id,
        };
        resources.entries.insert(id, (offer, body, entry_memory));
    }
    let _available_memory = budget.reserve(resources.entries.len() * 128 + 1024)?;
    let available = resources.entries.keys().copied().collect::<BTreeSet<_>>();
    let mut replica = Replica::with_memory_budget(limits, budget.clone());
    if !matches!(
        replica.apply_snapshot(&snapshot.bytes, &available)?,
        ApplyOutcome::Applied { .. }
    ) {
        return Err("snapshot resource closure is incomplete".into());
    }
    let mut viewport = Viewport::new(args[5].parse()?, args[6].parse()?);
    if args.len() == 8 {
        viewport.scale = args[7].parse()?;
    }
    let renderer = Renderer::new(RenderLimits::default(), budget)?;
    let frame = renderer.render_view(replica.state(), viewport, &resources)?;
    frame.write_pam(&mut File::create(&args[4])?)?;
    writeln!(
        std::io::stderr(),
        "{} revision={} work={}",
        PROFILE,
        frame.revision,
        frame.work
    )?;
    Ok(())
}
