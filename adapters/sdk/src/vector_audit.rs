//! Diagnostic audit of the actual decoded GTK publication/resource closure.
//! Source origins are cooperating-producer evidence, not remote attestation.
use crate::{hex, CaptureArtifact, CaptureError};
use rui_protocol_core::model::{
    Color, FillRule, GlyphFont, Primitive, Record, Rect, VisualCoverage,
};
use rui_protocol_core::receiver::ViewReceiver;
use rui_protocol_core::resources::{ResourceKind, ResourceProvenance};
use serde::Serialize;
use serde_json::{json, Value};
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
    /// Controlled full-visual fixture comparison after producer clock
    /// normalization. Audit the actual native capture strictly first, then
    /// compare every live field except independent publication/content clocks.
    /// This is not a projection/redaction policy or a native admission guard.
    pub fn verify_normalized_fixture_projection(
        &self,
        receiver: &ViewReceiver,
    ) -> Result<VectorAudit, CaptureError> {
        let mut source = self.receiver()?;
        self.apply_trusted_fixture(&mut source)?;
        let audit = self.verify_vector_audit(&source)?;
        fn content(value: &mut Value) {
            match value {
                Value::Object(fields) => {
                    for key in ["version", "text_version", "focus_generation"] {
                        fields.remove(key);
                    }
                    for field in fields.values_mut() {
                        content(field);
                    }
                }
                Value::Array(fields) => {
                    for field in fields {
                        content(field);
                    }
                }
                _ => {}
            }
        }
        let mut native = serde_json::to_value(&source.state()?.records)?;
        let mut public = serde_json::to_value(&receiver.state()?.records)?;
        content(&mut native);
        content(&mut public);
        if native != public || source.state()?.resource_refs() != receiver.state()?.resource_refs()
        {
            return Err(CaptureError::Invalid);
        }
        for digest in source.state()?.resource_refs() {
            if source.read_resource(digest)? != receiver.read_resource(digest)? {
                return Err(CaptureError::Invalid);
            }
        }
        Ok(audit)
    }
    fn verify_native_bindings(&self, receiver: &ViewReceiver) -> Result<(), CaptureError> {
        use rui_protocol_core::model::{ObjectKind, ObjectRef};
        let manifest = self.manifest();
        let state = receiver.state()?;
        let expected = manifest["visual_bindings"].as_array();
        let actual_count = state
            .records
            .values()
            .filter(|r| matches!(r, Record::Binding(_)))
            .count();
        if expected.map_or(0, Vec::len) != actual_count {
            return Err(CaptureError::Invalid);
        }
        let mut seen = BTreeSet::new();
        for source in expected.into_iter().flatten() {
            let id = source["id"].as_u64().ok_or(CaptureError::Invalid)?;
            let surface_id = source["surface_id"].as_u64().ok_or(CaptureError::Invalid)?;
            let semantic_id = source["semantic_id"]
                .as_u64()
                .ok_or(CaptureError::Invalid)?;
            let root = source["scene_root"].as_u64().ok_or(CaptureError::Invalid)?;
            let Some(Record::Binding(binding)) = state.records.get(&id) else {
                return Err(CaptureError::Invalid);
            };
            let Some(Record::Surface(surface)) = state.records.get(&surface_id) else {
                return Err(CaptureError::Invalid);
            };
            let Some(Record::Semantic(semantic)) = state.records.get(&semantic_id) else {
                return Err(CaptureError::Invalid);
            };
            let region = binding
                .interaction_region
                .map(|r| json!([r.x, r.y, r.width, r.height]))
                .unwrap_or(Value::Null);
            if !seen.insert(id)
                || source["version"].as_u64() != Some(binding.version)
                || binding.surface
                    != (ObjectRef {
                        kind: ObjectKind::Surface,
                        id: surface_id,
                    })
                || binding.semantics
                    != [ObjectRef {
                        kind: ObjectKind::Semantic,
                        id: semantic_id,
                    }]
                || binding.scenes
                    != [ObjectRef {
                        kind: ObjectKind::Scene,
                        id: root,
                    }]
                || surface.scene_root
                    != Some(ObjectRef {
                        kind: ObjectKind::Scene,
                        id: root,
                    })
                || !semantic.bindings.contains(&ObjectRef {
                    kind: ObjectKind::Binding,
                    id,
                })
                || !same_json(&source["interaction_region"], &region)
                || ((!surface.mapped || surface.visual_coverage != VisualCoverage::Complete)
                    && binding.interaction_region.is_some())
            {
                return Err(CaptureError::Invalid);
            }
            let regions = if manifest["surfaces"].is_array() {
                let surface_source = manifest["surfaces"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["surface_id"].as_u64() == Some(surface_id))
                    .ok_or(CaptureError::Invalid)?;
                let key = surface_source["surface_key"]
                    .as_str()
                    .ok_or(CaptureError::Invalid)?;
                &manifest["source_boundary_facts"]["surfaces"][key]["binding_regions"]
            } else {
                &manifest["binding_regions"]
            };
            let native = regions
                .as_array()
                .ok_or(CaptureError::Invalid)?
                .iter()
                .find(|r| r["id"].as_u64() == Some(id))
                .ok_or(CaptureError::Invalid)?;
            if native["semantic_id"].as_u64() != Some(semantic_id)
                || (surface.mapped
                    && surface.visual_coverage == VisualCoverage::Complete
                    && !same_json(&native["interaction_region"], &region))
            {
                return Err(CaptureError::Invalid);
            }
            if let Some(rect) = binding.interaction_region {
                if rect.x < 0.0
                    || rect.y < 0.0
                    || rect.width <= 0.0
                    || rect.height <= 0.0
                    || rect.x + rect.width > surface.content_size.width
                    || rect.y + rect.height > surface.content_size.height
                {
                    return Err(CaptureError::Invalid);
                }
            }
        }
        for record in state.records.values() {
            let Record::Surface(surface) = record else {
                continue;
            };
            let source = if let Some(surfaces) = manifest["surfaces"].as_array() {
                surfaces
                    .iter()
                    .find(|s| s["surface_id"].as_u64() == Some(surface.id))
                    .ok_or(CaptureError::Invalid)?
            } else if manifest["surface_id"].as_u64() == Some(surface.id) {
                manifest
            } else {
                continue;
            };
            let plans = if manifest["surfaces"].is_array() {
                let key = source["surface_key"]
                    .as_str()
                    .ok_or(CaptureError::Invalid)?;
                &manifest["source_boundary_facts"]["surfaces"][key]["binding_regions"]
            } else {
                &manifest["binding_regions"]
            };
            let plan_ids: Vec<_> = plans
                .as_array()
                .into_iter()
                .flatten()
                .map(|plan| plan["id"].as_u64().ok_or(CaptureError::Invalid))
                .collect::<Result<_, _>>()?;
            let published: BTreeSet<_> = expected
                .into_iter()
                .flatten()
                .filter(|binding| binding["surface_id"].as_u64() == Some(surface.id))
                .map(|binding| binding["id"].as_u64().ok_or(CaptureError::Invalid))
                .collect::<Result<_, _>>()?;
            if plan_ids.len() != published.len()
                || plan_ids.into_iter().collect::<BTreeSet<_>>() != published
            {
                return Err(CaptureError::Invalid);
            }
            let regions: Vec<_> = if surface.mapped
                && surface.visual_coverage == VisualCoverage::Complete
                && source["binding_regions"].is_array()
            {
                vec![json!([
                    0,
                    0,
                    surface.content_size.width,
                    surface.content_size.height
                ])]
            } else {
                Vec::new()
            };
            let actual: Vec<_> = surface
                .input_region
                .iter()
                .map(|r| json!([r.x, r.y, r.width, r.height]))
                .collect();
            if !same_json(&json!(regions), &json!(actual)) {
                return Err(CaptureError::Invalid);
            }
        }
        Ok(())
    }
    /// Check whole-view source barrier evidence against every decoded surface.
    /// This remains diagnostic cooperating-source evidence, not attestation.
    fn verify_whole_view(&self, receiver: &ViewReceiver) -> Result<(), CaptureError> {
        use rui_protocol_core::model::{CaptureConsistency, SurfaceRole};
        let m = self.manifest();
        let state = receiver.state()?;
        let surfaces = m["surfaces"].as_array().ok_or(CaptureError::Invalid)?;
        let nonce = m["capture_barrier_nonce"]
            .as_u64()
            .ok_or(CaptureError::Invalid)?;
        let facts = &m["source_boundary_facts"];
        let topology = facts["surfaces"].as_object().ok_or(CaptureError::Invalid)?;
        if m["capture_boundary"] != "source-thread-all-registered-surfaces-fresh-snapshot-barrier"
            || facts["incarnation"] != m["binding"]["epoch"]
            || facts["source_revision"] != m["source_revision"]
            || topology.len() > 4
            || topology.is_empty()
            || facts["visual_policy"]["animations"] != false
            || facts["visual_policy"]["cursor_blink"] != false
            || facts["visual_policy"]["controlled_static_canvas"] != true
        {
            return Err(CaptureError::Invalid);
        }
        let mut seen = BTreeSet::new();
        for source in surfaces {
            let key = source["surface_key"]
                .as_str()
                .ok_or(CaptureError::Invalid)?;
            let f = topology.get(key).ok_or(CaptureError::Invalid)?;
            let id = source["surface_id"].as_u64().ok_or(CaptureError::Invalid)?;
            let Some(Record::Surface(actual)) = state.records.get(&id) else {
                return Err(CaptureError::Invalid);
            };
            let role = match actual.role {
                SurfaceRole::Toplevel => 1,
                SurfaceRole::Popup => 2,
                SurfaceRole::Tooltip => 3,
                SurfaceRole::Shell => 4,
            };
            if !seen.insert(id)
                || f["alive"] != true
                || f["native_parent_matches"] != true
                || f["surface_id"] != id
                || source["capture_barrier_nonce"].as_u64() != Some(nonce)
                || source["revision"].as_u64() != Some(state.revision)
                || source["surface_role"] != role
                || source["mapped"] != actual.mapped
                || source["mapped"] != f["mapped"]
                || source["focused"] != actual.focused
                || source["stacking_order"].as_i64() != Some(actual.stacking_order)
                || source["stacking_order"] != f["stacking_order"]
                || !same_json(
                    &source["origin"],
                    &json!([actual.desktop_rect.x, actual.desktop_rect.y]),
                )
                || source["origin"] != f["origin"]
                || !same_json(
                    &source["size"],
                    &json!([actual.content_size.width, actual.content_size.height]),
                )
                || source["size"] != f["size"]
                || actual.desktop_rect.width != actual.content_size.width
                || actual.desktop_rect.height != actual.content_size.height
                || actual.transform
                    != (rui_protocol_core::model::Affine {
                        xx: 1.0,
                        yx: 0.0,
                        xy: 0.0,
                        yy: 1.0,
                        tx: 0.0,
                        ty: 0.0,
                    })
                || actual.opacity != 1.0
                || actual.clip.is_some()
                || actual.parent.map(|r| r.id) != f["parent"].as_u64()
                || actual.modal_for.map(|r| r.id) != f["modal_for"].as_u64()
                || f["native_modal"].as_bool().is_none()
                || actual.modal_for.is_some() != (actual.mapped && f["native_modal"] == true)
                || actual
                    .modal_for
                    .is_some_and(|r| Some(r.id) != actual.parent.map(|r| r.id))
                || actual.scene_root.map(|r| r.id) != source["scene_root"].as_u64()
                || actual.semantic_root.map(|r| r.id) != f["semantic_root"].as_u64()
                || actual.capture_consistency != CaptureConsistency::Coherent
                || actual.source_capture_tokens.len() != 1
                || actual.source_capture_tokens[0]
                    != source["capture_token"]
                        .as_str()
                        .ok_or(CaptureError::Invalid)?
                        .as_bytes()
                || (actual.mapped
                    && source["capture_boundary"] != "actual-snapshot-vfunc-same-node-append")
                || (!actual.mapped && source["capture_boundary"] != "source-lifecycle-state")
            {
                return Err(CaptureError::Invalid);
            }
            if !actual.mapped {
                let root = actual.scene_root.ok_or(CaptureError::Invalid)?;
                let Some(Record::Scene(scene)) = state.records.get(&root.id) else {
                    return Err(CaptureError::Invalid);
                };
                if !matches!(scene.primitive, Primitive::Group) || !scene.children.is_empty() {
                    return Err(CaptureError::Invalid);
                }
            }
        }
        if seen.len() != topology.values().filter(|f| f["alive"] == true).count()
            || seen.len()
                != state
                    .records
                    .values()
                    .filter(|r| matches!(r, Record::Surface(_)))
                    .count()
        {
            return Err(CaptureError::Invalid);
        }
        let session_id = m["session"]["id"].as_u64().ok_or(CaptureError::Invalid)?;
        let Some(Record::Session(session)) = state.records.get(&session_id) else {
            return Err(CaptureError::Invalid);
        };
        let (boundary_focus, boundary_surface) = if facts["focus"].is_null() {
            (None, None)
        } else {
            let focus = facts["focus"].as_array().ok_or(CaptureError::Invalid)?;
            if focus.len() != 2 {
                return Err(CaptureError::Invalid);
            }
            let key = focus[0].as_str().ok_or(CaptureError::Invalid)?;
            let surface = topology.get(key).ok_or(CaptureError::Invalid)?;
            if surface["alive"] != true || surface["mapped"] != true {
                return Err(CaptureError::Invalid);
            }
            (
                Some(focus[1].as_u64().ok_or(CaptureError::Invalid)?),
                Some(
                    surface["surface_id"]
                        .as_u64()
                        .ok_or(CaptureError::Invalid)?,
                ),
            )
        };
        if session.focus.map(|r| r.id) != boundary_focus
            || session.focus_surface.map(|r| r.id) != boundary_surface
            || session.focus.map(|r| r.id) != m["session"]["focus"].as_u64()
            || session.focus_surface.map(|r| r.id) != m["session"]["focus_surface"].as_u64()
            || Some(session.focus_generation) != m["session"]["focus_generation"].as_u64()
            || (m["native_context_profile"] == "gtk-native-focus-context.v1"
                && facts["focus_generation"].as_u64() != Some(session.focus_generation))
            || state
                .records
                .values()
                .filter(|r| matches!(r, Record::Session(_)))
                .count()
                != 1
        {
            return Err(CaptureError::Invalid);
        }
        Ok(())
    }

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
        let mut scene_resources = BTreeSet::new();
        let mut counts = BTreeMap::new();
        for source in scenes {
            let id = source["id"].as_u64().ok_or(CaptureError::Invalid)?;
            let Some(Record::Scene(scene)) = state.records.get(&id) else {
                return Err(CaptureError::Invalid);
            };
            let a = scene.transform;
            let children: Vec<_> = scene.children.iter().map(|r| r.id).collect();
            let p = primitive(&scene.primitive)?;
            if let Some(resource) = p.get("resource") {
                scene_resources.insert(hex::<32>(resource)?);
            }
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
                || !scene.resources.is_empty()
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
        if declared != closure
            || closure != scene_resources
            || declared.len() != listed.len()
            || offers.len() != closure.len()
        {
            return Err(CaptureError::Invalid);
        }
        let surfaces = manifest.get("surfaces").and_then(Value::as_array);
        if surfaces.is_some_and(|surfaces| surfaces.is_empty() || surfaces.len() > 4) {
            return Err(CaptureError::Invalid);
        }
        let local_draws = surfaces.map_or(u64::from(manifest["mapped"] == true), |surfaces| {
            surfaces
                .iter()
                .filter(|surface| surface["mapped"] == true)
                .count() as u64
        });
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
                || c["local_reference_draws"].as_u64() != Some(local_draws)
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
        let check_coverage = |source: &Value| -> Result<(), CaptureError> {
            let id = source["surface_id"].as_u64().ok_or(CaptureError::Invalid)?;
            let Some(Record::Surface(surface)) = state.records.get(&id) else {
                return Err(CaptureError::Invalid);
            };
            let gaps = source["gaps"].as_array().ok_or(CaptureError::Invalid)?;
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
            Ok(())
        };
        if let Some(surfaces) = surfaces {
            self.verify_whole_view(receiver)?;
            for surface in surfaces {
                check_coverage(surface)?;
            }
        } else if manifest["destroyed"] != true {
            check_coverage(manifest)?;
        }
        self.verify_native_bindings(receiver)?;
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
