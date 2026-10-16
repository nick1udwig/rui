//! Worker input is bounded data, never executable client content.
use rui_protocol_core::wire::proto as p;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: u64,
    pub version: u64,
    pub role: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub children: Vec<u64>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub disabled: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub nodes: Vec<Node>,
    pub status: String,
}
pub fn reference(id: u64) -> p::ObjectRef {
    p::ObjectRef {
        kind: p::ObjectKind::Semantic as i32,
        id,
    }
}
pub fn snapshot(capture: &Capture) -> Result<p::Snapshot, &'static str> {
    if capture.nodes.is_empty() || capture.nodes.len() > 1024 || capture.status.len() > 512 {
        return Err("capture bounds");
    }
    let ids: BTreeSet<_> = capture.nodes.iter().map(|n| n.id).collect();
    if ids.len() != capture.nodes.len() || !ids.contains(&3) {
        return Err("capture identities");
    }
    let mut records = Vec::new();
    let mut owners = BTreeMap::new();
    for n in &capture.nodes {
        if n.id < 3
            || n.version == 0
            || n.label.len() > 16384
            || n.description.len() > 16384
            || n.children.len() > 1024
            || n.actions.len() > 3
        {
            return Err("node bounds");
        }
        for child in &n.children {
            if !ids.contains(child) || *child == 3 || owners.insert(*child, n.id).is_some() {
                return Err("node ownership");
            }
        }
        let role = match n.role.as_str() {
            "root" => p::SemanticRole::Root,
            "group" => p::SemanticRole::GroupRole,
            "button" => p::SemanticRole::Button,
            "checkbox" => p::SemanticRole::Checkbox,
            "text" => p::SemanticRole::TextField,
            "label" => p::SemanticRole::Label,
            _ => return Err("role"),
        };
        if n.actions
            .iter()
            .any(|a| !matches!(a.as_str(), "Activate" | "Focus" | "SetValue"))
        {
            return Err("action");
        }
        if n.kind.as_ref().is_some_and(|k| {
            !matches!(
                k.as_str(),
                "text"
                    | "password"
                    | "email"
                    | "url"
                    | "search"
                    | "tel"
                    | "number"
                    | "date"
                    | "datetime-local"
                    | "time"
                    | "month"
                    | "week"
                    | "textarea"
                    | "link"
                    | "file"
                    | "radio"
                    | "range"
            )
        }) {
            return Err("input kind");
        }
        let value = match &n.value {
            Some(serde_json::Value::String(v)) if v.len() <= 16384 => {
                Some(p::typed_value::Value::Text(v.clone()))
            }
            Some(serde_json::Value::Bool(v)) => Some(p::typed_value::Value::Boolean(*v)),
            None => None,
            _ => return Err("value"),
        };
        if n.kind.as_deref() == Some("password")
            && n.value != Some(serde_json::Value::String(String::new()))
        {
            return Err("secret reflected");
        }
        records.push(p::Record {
            record: Some(p::record::Record::Semantic(p::SemanticNode {
                id: n.id,
                version: n.version,
                role: role as i32,
                label: Some(n.label.clone()),
                description: Some(n.description.clone()),
                typed_value: value.map(|value| p::TypedValue { value: Some(value) }),
                states: vec![p::StateValue {
                    key: p::StateKey::Disabled as i32,
                    value: if n.disabled {
                        p::TriState::True
                    } else {
                        p::TriState::False
                    } as i32,
                }],
                ordered_children: n.children.iter().copied().map(reference).collect(),
                actions: n
                    .actions
                    .iter()
                    .map(|a| p::Action {
                        id: a.clone(),
                        schema_version: 1,
                    })
                    .collect(),
                sensitivity: if n.kind.as_deref() == Some("password") {
                    p::Sensitivity::Sensitive
                } else {
                    p::Sensitivity::Public
                } as i32,
                completeness: p::Completeness::Incomplete as i32,
                browser_input_type: n.kind.clone(),
                ..Default::default()
            })),
        });
    }
    if owners.len() + 1 != ids.len() {
        return Err("orphan");
    }
    let by_id: BTreeMap<_, _> = capture.nodes.iter().map(|n| (n.id, n)).collect();
    let mut pending = vec![(3, 0)];
    let mut visited = BTreeSet::new();
    while let Some((id, depth)) = pending.pop() {
        if depth > 64 || !visited.insert(id) {
            return Err("cyclic or deep ownership");
        }
        pending.extend(by_id[&id].children.iter().map(|id| (*id, depth + 1)));
    }
    if visited.len() != ids.len() {
        return Err("disconnected ownership");
    }
    records.push(p::Record {
        record: Some(p::record::Record::Surface(p::Surface {
            id: 2,
            version: 1,
            role: p::SurfaceRole::Toplevel as i32,
            desktop_rect: Some(p::Rect {
                x: 0.,
                y: 0.,
                width: 1280.,
                height: 900.,
            }),
            content_size: Some(p::Size {
                width: 1280.,
                height: 900.,
            }),
            transform: Some(p::Affine {
                xx: 1.,
                yy: 1.,
                ..Default::default()
            }),
            opacity: 1.,
            mapped: true,
            semantic_root: Some(reference(3)),
            visual_coverage: p::VisualCoverage::Unsupported as i32,
            semantic_coverage: p::SemanticCoverage::SemanticPartial as i32,
            capture_consistency: p::CaptureConsistency::Observed as i32,
            source_capture_tokens: vec![
                b"browser-semantic-observation".to_vec(),
                b"no-visual-observation".to_vec(),
            ],
            unsupported_features: vec![capture.status.clone()],
            ..Default::default()
        })),
    });
    records.push(p::Record {
        record: Some(p::record::Record::Session(p::SessionState {
            id: 1,
            version: 1,
            canvas: Some(p::Rect {
                x: 0.,
                y: 0.,
                width: 1280.,
                height: 900.,
            }),
            focus_generation: 1,
            ..Default::default()
        })),
    });
    Ok(p::Snapshot {
        revision: 1,
        records,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture() -> Capture {
        Capture {
            nodes: vec![Node {
                id: 3,
                version: 1,
                role: "root".into(),
                label: "Browser".into(),
                description: String::new(),
                kind: None,
                value: None,
                children: vec![],
                actions: vec![],
                disabled: false,
            }],
            status: "Observed".into(),
        }
    }
    #[test]
    fn accepts_a_root_but_rejects_disconnected_cycles() {
        assert!(snapshot(&capture()).is_ok());
        let mut c = capture();
        let mut a = c.nodes[0].clone();
        a.id = 10;
        a.children = vec![11];
        let mut b = a.clone();
        b.id = 11;
        b.children = vec![10];
        c.nodes.extend([a, b]);
        assert!(snapshot(&c).is_err());
    }
    #[test]
    fn rejects_private_values_and_reused_ownership() {
        let mut c = capture();
        c.nodes[0].kind = Some("password".into());
        c.nodes[0].value = Some("secret".into());
        assert!(snapshot(&c).is_err());
        let mut c = capture();
        c.nodes[0].children = vec![3];
        assert!(snapshot(&c).is_err());
    }
}
