//! Diagnostic audit of the actual decoded GTK publication/resource closure.
//! Source origins are cooperating-producer evidence, not remote attestation.
use crate::{CaptureArtifact, CaptureError, hex};
use rui_protocol_core::model::{
    Color, FillRule, GlyphFont, Primitive, Record, Rect, VisualCoverage,
};
use rui_protocol_core::receiver::ViewReceiver;
use rui_protocol_core::resources::{ResourceKind, ResourceProvenance};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize)]
pub struct VectorAudit {
    pub scene_records: usize,
    pub primitive_counts: BTreeMap<String, usize>,
    pub resource_bodies: usize,
    pub resource_bytes: u64,
    pub unsupported_subtrees: usize,
    pub observed_counter_schema: bool,
}

fn paint(c: Color) -> Value {
    json!([c.red, c.green, c.blue, c.alpha])
}
fn rectangle(r: Rect) -> Value {
    json!([r.x, r.y, r.width, r.height])
}
fn digest(id: &[u8; 32]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
fn primitive(p: &Primitive) -> Result<Value, CaptureError> {
    Ok(match p {
        Primitive::Group => json!({"type":"Group"}),
        Primitive::SolidRect { rect, color } => {
            json!({"type":"SolidRect","rect":rectangle(*rect),"color":paint(*color)})
        }
        Primitive::FillPath {
            resource,
            color,
            fill_rule,
        } => {
            json!({"type":"FillPath","resource":digest(resource),"color":paint(*color),"fill_rule":match fill_rule {FillRule::Nonzero=>1,FillRule::EvenOdd=>2}})
        }
        Primitive::GlyphRun {
            font: GlyphFont::Outline(resource),
            size,
            glyph_ids,
            positions,
            scalar_clusters,
            color,
        } if scalar_clusters.is_empty() => {
            json!({"type":"GlyphRun","resource":digest(resource),"size":size,"glyph_ids":glyph_ids,"positions":positions.iter().map(|p|[p.x,p.y]).collect::<Vec<_>>(),"color":paint(*color)})
        }
        Primitive::RoundedRect { rect, radii, color }
        | Primitive::Border {
            rect, radii, color, ..
        } => {
            let corners = [
                radii.top_left,
                radii.top_right,
                radii.bottom_right,
                radii.bottom_left,
            ]
            .map(|s| [s.width, s.height]);
            if let Primitive::Border { width, .. } = p {
                json!({"type":"Border","rect":rectangle(*rect),"radii":corners,"width":width,"color":paint(*color)})
            } else {
                json!({"type":"RoundedRect","rect":rectangle(*rect),"radii":corners,"color":paint(*color)})
            }
        }
        Primitive::StrokePath {
            resource,
            color,
            width,
            cap,
            join,
            miter_limit,
            dash_lengths,
            dash_offset,
        } => {
            use rui_protocol_core::model::{StrokeCap, StrokeJoin};
            json!({"type":"StrokePath","resource":digest(resource),"color":paint(*color),"width":width,"cap":match cap {StrokeCap::Butt=>1,StrokeCap::Round=>2,StrokeCap::Square=>3},"join":match join {StrokeJoin::Miter=>1,StrokeJoin::Round=>2,StrokeJoin::Bevel=>3},"miter_limit":miter_limit,"dash_lengths":dash_lengths,"dash_offset":dash_offset})
        }
        _ => return Err(CaptureError::Invalid),
    })
}
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_json(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| same_json(a, b)))
        }
        _ => a == b,
    }
}

impl CaptureArtifact {
    /// Compare exact decoded scenes, closure and coverage to source evidence.
    /// Schema1 historical captures are auditable but lack observed origin events.
    pub fn verify_vector_audit(
        &self,
        receiver: &ViewReceiver,
    ) -> Result<VectorAudit, CaptureError> {
        let manifest = self.manifest();
        let state = receiver.state()?;
        let scenes = manifest["scene"].as_array().ok_or(CaptureError::Invalid)?;
        let offers = manifest["resource_offers"]
            .as_array()
            .ok_or(CaptureError::Invalid)?;
        let gaps = manifest["gaps"].as_array().ok_or(CaptureError::Invalid)?;
        if scenes.len() > 20_000 || offers.len() > 4096 || gaps.len() > 4096 {
            return Err(CaptureError::Invalid);
        }
        let mut seen = BTreeSet::new();
        let mut counts = BTreeMap::new();
        for source in scenes {
            let id = source["id"].as_u64().ok_or(CaptureError::Invalid)?;
            let Some(Record::Scene(scene)) = state.records.get(&id) else {
                return Err(CaptureError::Invalid);
            };
            let a = scene.transform;
            let children: Vec<_> = scene.children.iter().map(|r| r.id).collect();
            let p = primitive(&scene.primitive)?;
            if !seen.insert(id)
                || source["version"].as_u64() != Some(scene.version)
                || !same_json(&source["primitive"], &p)
                || !same_json(&source["children"], &json!(children))
                || !same_json(
                    &source["transform"],
                    &json!([a.xx, a.yx, a.xy, a.yy, a.tx, a.ty]),
                )
                || !same_json(
                    &source["clip"],
                    &scene.clip.map(rectangle).unwrap_or(Value::Null),
                )
                || source["opacity"].as_f64() != Some(scene.opacity)
            {
                return Err(CaptureError::Invalid);
            }
            *counts
                .entry(p["type"].as_str().ok_or(CaptureError::Invalid)?.to_owned())
                .or_insert(0) += 1;
        }
        if seen.len()
            != state
                .records
                .values()
                .filter(|r| matches!(r, Record::Scene(_)))
                .count()
        {
            return Err(CaptureError::Invalid);
        }
        let closure = state.resource_refs();
        let listed = manifest["resources"]
            .as_array()
            .ok_or(CaptureError::Invalid)?;
        let declared: BTreeSet<_> = listed.iter().map(hex::<32>).collect::<Result<_, _>>()?;
        if declared != closure || declared.len() != listed.len() || offers.len() != closure.len() {
            return Err(CaptureError::Invalid);
        }
        let schema2 = manifest["exporter_counters"]["schema"] == 2;
        let mut resource_bytes = 0u64;
        let mut kind_counts = BTreeMap::from([("1", 0usize), ("2", 0usize)]);
        let mut offer_ids = BTreeSet::new();
        for source in offers {
            let id = hex::<32>(&source["digest"])?;
            let offer = receiver.resource_offer(id)?;
            let (kind, provenance, origin_type) = match (offer.kind, offer.provenance) {
                (ResourceKind::VectorPath, ResourceProvenance::AuthoredVector) => (1, 1, "path"),
                (ResourceKind::GlyphOutline, ResourceProvenance::FontOutline) => {
                    (2, 2, "text-node")
                }
                _ => return Err(CaptureError::Invalid),
            };
            if !offer_ids.insert(id)
                || !closure.contains(&id)
                || source["kind"] != kind
                || source["provenance"] != provenance
                || source["version"].as_u64() != Some(u64::from(offer.version))
                || source["total_bytes"].as_u64() != Some(offer.total_bytes)
            {
                return Err(CaptureError::Invalid);
            }
            resource_bytes = resource_bytes
                .checked_add(offer.total_bytes)
                .ok_or(CaptureError::Invalid)?;
            *kind_counts
                .get_mut(if kind == 1 { "1" } else { "2" })
                .unwrap() += 1;
            if schema2 {
                let key = source["digest"].as_str().ok_or(CaptureError::Invalid)?;
                let origins = manifest["geometry_resource_origins"][key]
                    .as_array()
                    .ok_or(CaptureError::Invalid)?;
                if origins.is_empty() || origins.len() > 20_000 {
                    return Err(CaptureError::Invalid);
                };
                for origin in origins {
                    let expected = match origin["origin"].as_str() {
                        Some("native-pango-glyph-outline") if origin_type == "text-node" => {
                            "text-node"
                        }
                        Some("gsk-fill-path") if origin_type == "path" => "fill-node",
                        Some("gsk-stroke-path") if origin_type == "path" => "stroke-node",
                        _ => return Err(CaptureError::Invalid),
                    };
                    if !manifest["gsk_node_bounds"]
                        .as_array()
                        .ok_or(CaptureError::Invalid)?
                        .iter()
                        .any(|node| node["path"] == origin["node_path"] && node["type"] == expected)
                    {
                        return Err(CaptureError::Invalid);
                    }
                }
            }
        }
        if schema2 {
            let c = &manifest["exporter_counters"];
            let sum_counts = |value: &Value| -> Result<u64, CaptureError> {
                value
                    .as_object()
                    .ok_or(CaptureError::Invalid)?
                    .values()
                    .try_fold(0u64, |total, value| {
                        total
                            .checked_add(value.as_u64().ok_or(CaptureError::Invalid)?)
                            .ok_or(CaptureError::Invalid)
                    })
            };
            let visited = c["source_nodes_visited"]
                .as_u64()
                .ok_or(CaptureError::Invalid)?;
            if c["vector_records_emitted"].as_u64() != Some(scenes.len() as u64)
                || c["vector_resource_bodies"].as_u64() != Some(closure.len() as u64)
                || c["unsupported_subtrees"].as_u64() != Some(gaps.len() as u64)
                || !same_json(&c["vector_primitive_types"], &json!(counts))
                || manifest["geometry_resource_origins"]
                    .as_object()
                    .ok_or(CaptureError::Invalid)?
                    .len()
                    != closure.len()
                || manifest["protocol_handoff"]["resource_bytes"].as_u64() != Some(resource_bytes)
                || !same_json(
                    &manifest["protocol_handoff"]["resource_kind_counts"],
                    &json!(kind_counts),
                )
                || sum_counts(&c["source_node_types"])? != visited
                || sum_counts(&c["node_decisions"])? > visited
                || sum_counts(&c["decline_reasons"])? != gaps.len() as u64
                || c["local_reference_draws"].as_u64()
                    != Some(u64::from(manifest["mapped"] == true))
                || manifest["protocol_handoff"]["envelope_bytes"].as_u64()
                    != Some(std::fs::metadata(self.path.with_extension("bin"))?.len())
            {
                return Err(CaptureError::Invalid);
            }
            let census = manifest["gsk_node_bounds"]
                .as_array()
                .ok_or(CaptureError::Invalid)?;
            for gap in gaps {
                if !gap["path"].is_string()
                    || !gap["node_type"].is_string()
                    || !census.iter().any(|node| {
                        node["path"] == gap["path"]
                            && node["type"] == gap["node_type"]
                            && same_json(&node["bounds"], &gap["bounds"])
                    })
                {
                    return Err(CaptureError::Invalid);
                }
            }
            if let Some(probe) = manifest.get("fallback_probe") {
                if !probe["forbidden_export_calls"]
                    .as_object()
                    .ok_or(CaptureError::Invalid)?
                    .is_empty()
                {
                    return Err(CaptureError::Invalid);
                }
            }
        }
        if manifest["destroyed"] != true {
            let id = manifest["surface_id"]
                .as_u64()
                .ok_or(CaptureError::Invalid)?;
            let Some(Record::Surface(surface)) = state.records.get(&id) else {
                return Err(CaptureError::Invalid);
            };
            let reasons: BTreeSet<_> = gaps
                .iter()
                .map(|g| g["reason"].as_str().ok_or(CaptureError::Invalid))
                .collect::<Result<_, _>>()?;
            let actual: BTreeSet<_> = surface
                .unsupported_features
                .iter()
                .map(String::as_str)
                .collect();
            if actual != reasons
                || surface.visual_coverage
                    != if gaps.is_empty() {
                        VisualCoverage::Complete
                    } else {
                        VisualCoverage::Partial
                    }
            {
                return Err(CaptureError::Invalid);
            }
        }
        self.verify_native_glyph_evidence(receiver)?;
        Ok(VectorAudit {
            scene_records: scenes.len(),
            primitive_counts: counts,
            resource_bodies: closure.len(),
            resource_bytes,
            unsupported_subtrees: gaps.len(),
            observed_counter_schema: schema2,
        })
    }
}
