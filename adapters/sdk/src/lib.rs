//! Bounded capture artifact consumer. This is a local fixture bridge, not a
//! network or authenticated broker. Trust policy is supplied by the caller.
pub mod gtk_host;
mod native_fence;
mod source_identity;
pub mod vector_audit;
use rui_protocol_core::model::{GlyphFont, Primitive, Record};
use rui_protocol_core::receiver::{ReceiveOutcome, ReceiverConfig, ReceiverError, ViewReceiver};
use rui_protocol_core::resources::{
    OfferOutcome, ResourceContext, ResourceKind, ResourceOffer, ResourceProvenance, ResourceScope,
};
use rui_protocol_core::session::ReceiverLimits;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_ENVELOPE_BYTES: u64 = 1 << 20;
const MAX_RESOURCE_BYTES: u64 = 16 << 20;
const MAX_MANIFEST_ALLOCATION: usize = 16 << 20;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture artifact is malformed or exceeds limits")]
    Invalid,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Receiver(#[from] ReceiverError),
}

fn bounded_read(path: &Path, maximum: u64) -> Result<Vec<u8>, CaptureError> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > maximum {
        return Err(CaptureError::Invalid);
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(CaptureError::Invalid);
    }
    Ok(bytes)
}

/// Allocation-free lexical preflight before serde constructs arrays/maps.
/// Includes sparse BTreeMap roots, Vec spare capacity and decoded strings.
/// Full token/UTF-8 validity is still checked by serde after this quota check.
fn manifest_preflight(bytes: &[u8]) -> Result<(), CaptureError> {
    let mut allocation = bytes.len().checked_mul(4).ok_or(CaptureError::Invalid)?;
    let mut stack = [0u8; 64];
    let mut depth = 0;
    let mut quoted = false;
    let mut escaped = false;
    for byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
            continue;
        }
        if *byte == b'"' {
            quoted = true;
        }
        if matches!(byte, b'[' | b'{' | b']' | b'}' | b',' | b':') {
            allocation = allocation.checked_add(256).ok_or(CaptureError::Invalid)?;
            if allocation > MAX_MANIFEST_ALLOCATION {
                return Err(CaptureError::Invalid);
            }
        }
        match byte {
            b'[' | b'{' => {
                if depth == stack.len() {
                    return Err(CaptureError::Invalid);
                }
                stack[depth] = *byte;
                depth += 1;
            }
            b']' | b'}' => {
                if depth == 0 || !matches!((stack[depth - 1], *byte), (b'[', b']') | (b'{', b'}')) {
                    return Err(CaptureError::Invalid);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    if quoted || depth != 0 || allocation > MAX_MANIFEST_ALLOCATION {
        return Err(CaptureError::Invalid);
    }
    Ok(())
}

fn hex<const N: usize>(value: &Value) -> Result<[u8; N], CaptureError> {
    let text = value.as_str().ok_or(CaptureError::Invalid)?;
    if text.len() != N * 2 || !text.is_ascii() {
        return Err(CaptureError::Invalid);
    }
    let mut result = [0; N];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| CaptureError::Invalid)?;
    }
    Ok(result)
}

pub struct CaptureArtifact {
    manifest: Value,
    path: PathBuf,
    context: ResourceContext,
}

impl CaptureArtifact {
    pub fn read(path: impl AsRef<Path>) -> Result<Self, CaptureError> {
        let path = path.as_ref().to_path_buf();
        let bytes = bounded_read(&path, MAX_MANIFEST_BYTES)?;
        manifest_preflight(&bytes)?;
        let manifest: Value = serde_json::from_slice(&bytes)?;
        if !matches!(
            manifest["capture_boundary"].as_str(),
            Some(
                "actual-snapshot-vfunc-same-node-append"
                    | "source-lifecycle-signal"
                    | "source-thread-all-registered-surfaces-fresh-snapshot-barrier"
            )
        ) || manifest["gtk_version"] != serde_json::json!([4, 22, 4])
        {
            return Err(CaptureError::Invalid);
        }
        let binding = &manifest["binding"];
        let context = ResourceContext {
            scope: ResourceScope {
                session_id: hex(&binding["session_id"])?,
                epoch: hex(&binding["epoch"])?,
                principal_id: [3; 16],
            },
            view_id: hex(&binding["view_id"])?,
        };
        Ok(Self {
            manifest,
            path,
            context,
        })
    }
    pub fn context(&self) -> ResourceContext {
        self.context
    }
    pub fn manifest(&self) -> &Value {
        &self.manifest
    }
    /// Compare native source glyph evidence against the actual decoded state.
    /// This is a diagnostic consistency check, not proof that a manifest came
    /// from GTK. It must pass before a fixture renderer presents this capture.
    pub fn verify_native_glyph_evidence(
        &self,
        receiver: &ViewReceiver,
    ) -> Result<usize, CaptureError> {
        let evidence = self.manifest["source_glyph_evidence"]
            .as_array()
            .ok_or(CaptureError::Invalid)?;
        if evidence.len() > 20_000 {
            return Err(CaptureError::Invalid);
        }
        let state = receiver.state()?;
        let actual_count = state.records.values().filter(|record| {
            matches!(record, Record::Scene(scene) if matches!(scene.primitive, Primitive::GlyphRun { .. }))
        }).count();
        if evidence.len() != actual_count {
            return Err(CaptureError::Invalid);
        }
        let number = |value: &Value| {
            value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or(CaptureError::Invalid)
        };
        let mut seen = BTreeSet::new();
        for glyph in evidence {
            let id = glyph["scene_id"].as_u64().ok_or(CaptureError::Invalid)?;
            if !seen.insert(id) {
                return Err(CaptureError::Invalid);
            }
            let Some(Record::Scene(scene)) = state.records.get(&id) else {
                return Err(CaptureError::Invalid);
            };
            let Primitive::GlyphRun {
                font: GlyphFont::Outline(resource),
                size,
                glyph_ids,
                positions,
                ..
            } = &scene.primitive
            else {
                return Err(CaptureError::Invalid);
            };
            let scale = number(&glyph["pango_scale"])?;
            let x = number(&glyph["node_offset"][0])?
                + number(&glyph["advance"])?
                + number(&glyph["pango_x_offset"])? / scale;
            let y = number(&glyph["node_offset"][1])? + number(&glyph["pango_y_offset"])? / scale;
            if scale != 1024.0
                || *resource != hex::<32>(&glyph["resource"])?
                || *size != number(&glyph["font_size"])?
                || glyph_ids != &[0]
                || positions.len() != 1
                || positions[0].x != x
                || positions[0].y != y
            {
                return Err(CaptureError::Invalid);
            }
        }
        Ok(evidence.len())
    }
    pub fn receiver(&self) -> Result<ViewReceiver, CaptureError> {
        let config = ReceiverConfig {
            single_glyph_outlines: true,
            ..ReceiverConfig::default()
        };
        Ok(ViewReceiver::new(
            self.context,
            ReceiverLimits::default(),
            config,
        )?)
    }
    /// Caller has approved this local fixture's exact projection/resource policy.
    /// This method never grants an untrusted external producer access by digest.
    pub fn apply_trusted_fixture(&self, receiver: &mut ViewReceiver) -> Result<u64, CaptureError> {
        let offers = self.manifest["resource_offers"]
            .as_array()
            .ok_or(CaptureError::Invalid)?;
        if offers.len() > 4096 {
            return Err(CaptureError::Invalid);
        }
        let mut seen = BTreeSet::new();
        let mut resources = Vec::new();
        let directory = self.path.parent().ok_or(CaptureError::Invalid)?;
        for value in offers {
            let digest = hex::<32>(&value["digest"])?;
            if !seen.insert(digest) {
                return Err(CaptureError::Invalid);
            }
            let digest_text = value["digest"].as_str().ok_or(CaptureError::Invalid)?;
            let length = fs::metadata(directory.join(format!("{digest_text}.rvp")))?.len();
            let declared = value["total_bytes"].as_u64().ok_or(CaptureError::Invalid)?;
            if length > MAX_RESOURCE_BYTES || length != declared {
                return Err(CaptureError::Invalid);
            }
            let kind = ResourceKind::try_from(
                value["kind"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or(CaptureError::Invalid)?,
            )
            .map_err(|_| CaptureError::Invalid)?;
            let provenance = ResourceProvenance::try_from(
                value["provenance"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or(CaptureError::Invalid)?,
            )
            .map_err(|_| CaptureError::Invalid)?;
            let version = value["version"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or(CaptureError::Invalid)?;
            resources.push(ResourceOffer {
                digest,
                kind,
                provenance,
                version,
                total_bytes: declared,
            });
        }
        // Resourceful publication must enter pending before bodies are offered.
        for offer in &resources {
            receiver.authorize_resource(offer.digest)?;
        }
        let revision = self.manifest["revision"]
            .as_u64()
            .ok_or(CaptureError::Invalid)?;
        let outcome = receiver.receive_envelope(
            (revision as u128).to_be_bytes(),
            &bounded_read(&self.path.with_extension("bin"), MAX_ENVELOPE_BYTES)?,
        )?;
        let applied = match outcome {
            ReceiveOutcome::Applied { revision } | ReceiveOutcome::Duplicate { revision } => {
                Some(revision)
            }
            ReceiveOutcome::NeedResources { missing, .. } => {
                drop(missing);
                None
            }
        };
        if let Some(applied) = applied {
            return Ok(applied);
        }
        let mut applied = None;
        for offer in resources {
            let digest_text = offer
                .digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let bytes = bounded_read(
                &directory.join(format!("{digest_text}.rvp")),
                MAX_RESOURCE_BYTES,
            )?;
            if bytes.len() as u64 != offer.total_bytes {
                return Err(CaptureError::Invalid);
            }
            if receiver.receive_resource_offer(self.context, offer.clone())?
                == OfferOutcome::Receiving
            {
                for (index, chunk) in bytes.chunks(64 * 1024).enumerate() {
                    receiver.resource_chunk(self.context, offer.digest, index as u32, chunk)?;
                }
            }
            match receiver.complete_resource(self.context, offer.digest)? {
                Some(ReceiveOutcome::Applied { revision }) => applied = Some(revision),
                Some(ReceiveOutcome::NeedResources { missing, .. }) => drop(missing),
                _ => {}
            }
        }
        applied.ok_or(CaptureError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_preflight_rejects_deep_and_dense_manifests_before_deserialization() {
        let deep = format!("{}0{}", "[".repeat(65), "]".repeat(65));
        assert!(matches!(
            manifest_preflight(deep.as_bytes()),
            Err(CaptureError::Invalid)
        ));
        let dense = format!("[{}0]", "0,".repeat(70_000));
        assert!(dense.len() < MAX_MANIFEST_BYTES as usize);
        assert!(matches!(
            manifest_preflight(dense.as_bytes()),
            Err(CaptureError::Invalid)
        ));
        assert!(manifest_preflight(br#"{"string":"[{]}","nested":[{"value":1}]}"#).is_ok());
        assert!(manifest_preflight(b"[}").is_err());
    }
}
