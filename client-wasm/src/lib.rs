//! Browser bindings for the portable Rust receiver and shared session actor.
//! The low-level Receiver takes trusted construction/resource inputs; Session
//! validates binary session packets and builds guarded commands. Callers own
//! authenticated socket I/O, clocks and truthful presentation notifications.
use rui_protocol_core::{
    model::Record,
    receiver::{NeededResources, ReceiveOutcome, ReceiverConfig, ViewReceiver},
    resources::{ResourceContext, ResourceKind, ResourceOffer, ResourceProvenance, ResourceScope},
    session::ReceiverLimits,
};
use wasm_bindgen::prelude::*;

fn error(value: impl std::fmt::Display) -> JsError {
    JsError::new(&value.to_string())
}

fn id<const N: usize>(bytes: &[u8]) -> Result<[u8; N], JsError> {
    bytes
        .try_into()
        .map_err(|_| error(format!("expected {N} bytes")))
}

#[wasm_bindgen]
pub enum ReceiveStatus {
    Applied,
    Duplicate,
    NeedResources,
}

/// Owns an authentic Rust ViewReceiver; no second reducer exists in JavaScript.
#[wasm_bindgen]
pub struct Receiver {
    context: ResourceContext,
    receiver: ViewReceiver,
    missing: Option<NeededResources>,
}

#[wasm_bindgen]
impl Receiver {
    /// Supply bindings from a trusted, authenticated session integration.
    /// Glyph opt-in requires a negotiated profile and a compatible renderer.
    #[wasm_bindgen(constructor)]
    pub fn new(
        session: &[u8],
        epoch: &[u8],
        principal: &[u8],
        view: &[u8],
        memory_limit: u32,
        single_glyph_outlines: bool,
    ) -> Result<Receiver, JsError> {
        let context = ResourceContext {
            scope: ResourceScope {
                session_id: id(session)?,
                epoch: id(epoch)?,
                principal_id: id(principal)?,
            },
            view_id: id(view)?,
        };
        let receiver = ViewReceiver::new(
            context,
            ReceiverLimits {
                view_memory_bytes: memory_limit as usize,
                ..ReceiverLimits::default()
            },
            ReceiverConfig {
                single_glyph_outlines,
                ..ReceiverConfig::default()
            },
        )
        .map_err(error)?;
        Ok(Self {
            context,
            receiver,
            missing: None,
        })
    }

    /// Exact, bound core Envelope bytes, not a session Packet or reserialization.
    #[wasm_bindgen(js_name = receiveEnvelope)]
    pub fn receive_envelope(
        &mut self,
        publication: &[u8],
        bytes: &[u8],
    ) -> Result<ReceiveStatus, JsError> {
        let outcome = self.receiver.receive_envelope(id(publication)?, bytes);
        match outcome {
            Ok(outcome) => Ok(self.observe(outcome)),
            Err(value) => {
                if !self.receiver.has_pending_publication() {
                    self.missing = None;
                }
                Err(error(value))
            }
        }
    }

    #[wasm_bindgen(getter)]
    pub fn revision(&self) -> Result<u64, JsError> {
        Ok(self.receiver.state().map_err(error)?.revision)
    }

    /// Owned serialization of all six validated record variants.
    pub fn state(&self) -> Result<JsValue, JsError> {
        value(self.receiver.state().map_err(error)?)
    }

    #[wasm_bindgen(js_name = recordIds)]
    pub fn record_ids(&self) -> Result<Vec<u64>, JsError> {
        Ok(self
            .receiver
            .state()
            .map_err(error)?
            .records
            .keys()
            .copied()
            .collect())
    }

    #[wasm_bindgen(js_name = recordVersion)]
    pub fn record_version(&self, record: u64) -> Result<Option<u64>, JsError> {
        Ok(self
            .receiver
            .state()
            .map_err(error)?
            .records
            .get(&record)
            .map(Record::version))
    }

    #[wasm_bindgen(js_name = textContent)]
    pub fn text_content(&self, record: u64) -> Result<Option<String>, JsError> {
        Ok(
            match self.receiver.state().map_err(error)?.records.get(&record) {
                Some(Record::Text(text)) => text.content.clone(),
                _ => None,
            },
        )
    }

    #[wasm_bindgen(js_name = missingResources)]
    pub fn missing_resources(&self) -> Vec<u8> {
        self.missing
            .as_ref()
            .map(|missing| missing.iter().flat_map(|digest| *digest).collect())
            .unwrap_or_default()
    }

    /// Trusted local resource policy; an untrusted peer cannot grant itself access.
    #[wasm_bindgen(js_name = authorizeResource)]
    pub fn authorize_resource(&mut self, digest: &[u8]) -> Result<(), JsError> {
        self.receiver.authorize_resource(id(digest)?).map_err(error)
    }

    #[wasm_bindgen(js_name = resourceOffer)]
    pub fn resource_offer(
        &mut self,
        digest: &[u8],
        kind: u32,
        version: u32,
        provenance: u32,
        total_bytes: u64,
    ) -> Result<(), JsError> {
        let offer = ResourceOffer {
            digest: id(digest)?,
            kind: ResourceKind::try_from(kind).map_err(error)?,
            version,
            provenance: ResourceProvenance::try_from(provenance).map_err(error)?,
            total_bytes,
        };
        self.receiver
            .receive_resource_offer(self.context, offer)
            .map_err(error)?;
        Ok(())
    }

    #[wasm_bindgen(js_name = resourceChunk)]
    pub fn resource_chunk(
        &mut self,
        digest: &[u8],
        index: u32,
        bytes: &[u8],
    ) -> Result<(), JsError> {
        self.receiver
            .resource_chunk(self.context, id(digest)?, index, bytes)
            .map_err(error)?;
        Ok(())
    }

    #[wasm_bindgen(js_name = resourceComplete)]
    pub fn resource_complete(&mut self, digest: &[u8]) -> Result<Option<ReceiveStatus>, JsError> {
        let outcome = self
            .receiver
            .complete_resource(self.context, id(digest)?)
            .map_err(error)?;
        Ok(outcome.map(|outcome| self.observe(outcome)))
    }

    /// A bounded copy; JavaScript-owned bytes are outside Rust's allocation ledger.
    #[wasm_bindgen(js_name = resourceBytes)]
    pub fn resource_bytes(&self, digest: &[u8]) -> Result<Vec<u8>, JsError> {
        Ok(self
            .receiver
            .read_resource(id(digest)?)
            .map_err(error)?
            .to_vec())
    }

    #[wasm_bindgen(getter, js_name = memoryUsed)]
    pub fn memory_used(&self) -> usize {
        self.receiver.memory_used()
    }

    pub fn disconnect(&mut self) {
        self.missing = None;
        self.receiver.disconnect();
    }

    /// Call only after the same session has been reauthenticated and authorized.
    pub fn reconnect(&mut self) -> Result<(), JsError> {
        self.receiver.reconnect(self.context).map_err(error)
    }

    pub fn close(&mut self) {
        self.missing = None;
        self.receiver.close();
    }
}

impl Receiver {
    fn observe(&mut self, outcome: ReceiveOutcome) -> ReceiveStatus {
        match outcome {
            ReceiveOutcome::Applied { .. } => {
                self.missing = None;
                ReceiveStatus::Applied
            }
            ReceiveOutcome::Duplicate { .. } => ReceiveStatus::Duplicate,
            ReceiveOutcome::NeedResources { missing, .. } => {
                self.missing = Some(missing);
                ReceiveStatus::NeedResources
            }
        }
    }
}

/// Session packets, negotiations and commands use the same Rust codec as the
/// native TLS client. JavaScript supplies a trusted WebSocket transport.
#[wasm_bindgen]
pub struct Session {
    inner: rui_client_core::session::Session,
}
fn value<T: serde::Serialize>(input: &T) -> Result<JsValue, JsError> {
    input
        .serialize(
            &serde_wasm_bindgen::Serializer::new()
                .serialize_large_number_types_as_bigints(true)
                .serialize_missing_as_null(true),
        )
        .map_err(error)
}
#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(constructor)]
    pub fn new(
        client: &[u8],
        selection: String,
        semantic_only: Option<bool>,
    ) -> Result<Session, JsError> {
        Ok(Self {
            inner: rui_client_core::session::Session::with_mode(
                id(client)?,
                selection,
                if semantic_only.unwrap_or(false) {
                    rui_client_core::session::PresentationMode::SemanticOnly
                } else {
                    rui_client_core::session::PresentationMode::Mirror
                },
            )
            .map_err(error)?,
        })
    }
    pub fn start(&mut self, credential: &[u8]) -> Result<(), JsError> {
        self.inner.start(credential).map_err(error)
    }
    pub fn receive(&mut self, bytes: &[u8]) -> Result<JsValue, JsError> {
        value(&self.inner.receive(bytes).map_err(error)?)
    }
    #[wasm_bindgen(js_name = nextOutbound)]
    pub fn next_outbound(&mut self) -> Option<Vec<u8>> {
        self.inner.next_outbound()
    }
    #[wasm_bindgen(getter)]
    pub fn ceiling(&self) -> usize {
        self.inner.ceiling()
    }
    #[wasm_bindgen(getter)]
    pub fn revision(&self) -> u64 {
        self.inner.state().map_or(0, |s| s.revision)
    }
    #[wasm_bindgen(getter, js_name = hasControl)]
    pub fn has_control(&self) -> bool {
        self.inner.has_control()
    }
    #[wasm_bindgen(getter, js_name = pendingCommands)]
    pub fn pending_commands(&self) -> usize {
        self.inner.pending_commands()
    }
    #[wasm_bindgen(getter, js_name = memoryUsed)]
    pub fn memory_used(&self) -> usize {
        self.inner.memory_used()
    }
    /// Copy only the committed graph. IDs, clocks and scalar indices are BigInt.
    pub fn state(&self) -> Result<JsValue, JsError> {
        value(self.inner.state().map_err(error)?)
    }
    #[wasm_bindgen(js_name = resourceBytes)]
    pub fn resource_bytes(&self, digest: &[u8]) -> Result<Vec<u8>, JsError> {
        Ok(self
            .inner
            .receiver()
            .map_err(error)?
            .read_resource(id(digest)?)
            .map_err(error)?
            .to_vec())
    }
    pub fn capabilities(&self) -> Result<JsValue, JsError> {
        value(&self.inner.capabilities())
    }
    #[wasm_bindgen(js_name = semanticReady)]
    pub fn semantic_ready(&mut self, revision: u64) -> Result<(), JsError> {
        self.inner.semantic_ready(revision).map_err(error)
    }
    pub fn presented(&mut self, revision: u64) -> Result<(), JsError> {
        self.inner.presented(revision).map_err(error)
    }
    pub fn acquire(&mut self, now: u64) -> Result<(), JsError> {
        self.inner.acquire(now).map_err(error)
    }
    pub fn release(&mut self) -> Result<(), JsError> {
        self.inner.release().map_err(error)
    }
    pub fn tick(&mut self, now: u64) -> Result<(), JsError> {
        self.inner.tick(now).map_err(error)
    }
    #[allow(clippy::too_many_arguments)] // Flat primitive WASM ABI avoids unvalidated JS objects.
    #[wasm_bindgen(js_name = hitTest)]
    pub fn hit_test(
        &self,
        x: f64,
        y: f64,
        width: u32,
        height: u32,
        origin_x: f64,
        origin_y: f64,
        scale: f64,
    ) -> Result<u64, JsError> {
        self.inner
            .hit_test(
                rui_client_core::Viewport {
                    width,
                    height,
                    origin: rui_protocol_core::model::Point {
                        x: origin_x,
                        y: origin_y,
                    },
                    scale,
                },
                rui_protocol_core::model::Point { x, y },
            )
            .map_err(error)
    }
    pub fn activate(&mut self, target: u64) -> Result<u64, JsError> {
        self.inner
            .command(
                target,
                "Activate",
                rui_protocol_core::commands::Arguments::None,
            )
            .map_err(error)
    }
    pub fn focus(&mut self, target: u64) -> Result<u64, JsError> {
        self.inner
            .command(
                target,
                "Focus",
                rui_protocol_core::commands::Arguments::None,
            )
            .map_err(error)
    }
    #[wasm_bindgen(js_name = editToken)]
    pub fn edit_token(&self, target: u64) -> Result<Vec<u8>, JsError> {
        Ok(self.inner.edit_token(target).map_err(error)?.to_vec())
    }
    /// The UI must preserve these capture stamps while a user edits a draft.
    #[wasm_bindgen(js_name = replaceText)]
    pub fn replace_text(
        &mut self,
        target: u64,
        revision: u64,
        text_version: u64,
        text: String,
        token: &[u8],
    ) -> Result<u64, JsError> {
        if token != self.inner.edit_token(target).map_err(error)?.as_slice() {
            return Err(error("editing context changed"));
        }
        if self.inner.state().map_err(error)?.revision != revision {
            return Err(error("editing context changed"));
        }
        let model = self
            .inner
            .text(target)
            .ok_or_else(|| error("text is unavailable"))?;
        if model.text_version != text_version {
            return Err(error("editing context changed"));
        }
        let end = model
            .content
            .as_ref()
            .ok_or_else(|| error("text is unavailable"))?
            .chars()
            .count() as u64;
        self.inner
            .command(
                target,
                "ReplaceText",
                rui_protocol_core::commands::Arguments::ReplaceText {
                    start: 0,
                    end,
                    text,
                },
            )
            .map_err(error)
    }
    #[wasm_bindgen(js_name = setSelection)]
    pub fn set_selection(&mut self, target: u64, anchor: u64, focus: u64) -> Result<u64, JsError> {
        self.inner
            .command(
                target,
                "SetSelection",
                rui_protocol_core::commands::Arguments::Selection { anchor, focus },
            )
            .map_err(error)
    }
    #[wasm_bindgen(js_name = retryPending)]
    pub fn retry_pending(&mut self) -> Result<(), JsError> {
        self.inner.retry_pending().map_err(error)
    }
    pub fn disconnect(&mut self) {
        self.inner.disconnect();
    }
    pub fn close(&mut self) {
        self.inner.close();
    }
}
