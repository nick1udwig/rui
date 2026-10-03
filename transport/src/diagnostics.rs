//! Opt-in local diagnostics. Neither pairing credentials nor a wire request can
//! enable exact recording. Entries borrow the authenticated connection's scope
//! and disappear when it closes or its immutable approval is revoked.
use crate::{proto as p, TransportError};
use rui_host_broker::AuthorizationLifetime;
use rui_protocol_core::{
    resources::{MemoryBudget, ResourceContext, ResourceScope},
    session::{NegotiatedSession, ViewMemoryReservation},
    transcript::{
        Direction, RecordedMessage, RecordingPolicy, TranscriptLimits, TranscriptRecorder,
    },
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

struct Entry {
    context: ResourceContext,
    lifetime: AuthorizationLifetime,
    recorder: TranscriptRecorder,
    _memory: ViewMemoryReservation,
}
struct State {
    next: u64,
    entries: BTreeMap<u64, Entry>,
}
/// Trusted local diagnostic owner. Inspect via a borrowed callback so retained
/// wire buffers cannot escape their charge accidentally. A caller explicitly
/// copying bytes owns that separate diagnostic export and its retention policy.
pub struct Diagnostics {
    policy: RecordingPolicy,
    limits: TranscriptLimits,
    max_sessions: usize,
    state: Mutex<State>,
}
impl Diagnostics {
    pub fn new(
        policy: RecordingPolicy,
        limits: TranscriptLimits,
        max_sessions: usize,
    ) -> Result<Arc<Self>, TransportError> {
        if max_sessions == 0
            || max_sessions > 128
            || limits.records == 0
            || limits.records > 16384
            || limits.message_bytes == 0
            || limits.message_bytes > 1024 * 1024
            || limits.retained_bytes == 0
            || limits.retained_bytes > 64 * 1024 * 1024
        {
            return Err(TransportError::Limit);
        }
        Ok(Arc::new(Self {
            policy,
            limits,
            max_sessions,
            state: Mutex::new(State {
                next: 0,
                entries: BTreeMap::new(),
            }),
        }))
    }
    pub(crate) fn reservation_bytes(&self) -> usize {
        self.limits.retained_bytes + 4096
    }
    fn prune(state: &mut State) {
        state
            .entries
            .retain(|_, entry| entry.lifetime.is_authorized());
    }
    pub(crate) fn open(
        self: &Arc<Self>,
        session: &NegotiatedSession,
        lifetime: AuthorizationLifetime,
        memory: ViewMemoryReservation,
    ) -> Result<RecordingSession, TransportError> {
        let mut state = self.state.lock().map_err(|_| TransportError::Authority)?;
        Self::prune(&mut state);
        if !lifetime.is_authorized() {
            return Err(TransportError::Authentication);
        }
        if state.entries.len() >= self.max_sessions {
            return Err(TransportError::Limit);
        }
        let id = state.next.checked_add(1).ok_or(TransportError::Limit)?;
        let context = ResourceContext {
            scope: ResourceScope {
                session_id: session.session_id,
                epoch: session.epoch,
                principal_id: Sha256::digest(session.client.principal.as_bytes())[..16]
                    .try_into()
                    .unwrap(),
            },
            view_id: session.view_id,
        };
        // The outer reservation charges this entire bounded sub-arena to the
        // same host view, including after publication/history changes.
        let recorder = TranscriptRecorder::new(
            context,
            self.policy,
            self.limits.clone(),
            MemoryBudget::new(self.limits.retained_bytes),
        )
        .map_err(|_| TransportError::Limit)?;
        state.entries.insert(
            id,
            Entry {
                context,
                lifetime,
                recorder,
                _memory: memory,
            },
        );
        state.next = id;
        Ok(RecordingSession {
            owner: self.clone(),
            id,
        })
    }
    pub fn scopes(&self) -> Result<Vec<ResourceContext>, TransportError> {
        let mut state = self.state.lock().map_err(|_| TransportError::Authority)?;
        Self::prune(&mut state);
        Ok(state.entries.values().map(|e| e.context).collect())
    }
    pub fn inspect<T>(
        &self,
        context: ResourceContext,
        inspect: impl FnOnce(&mut dyn Iterator<Item = &RecordedMessage>) -> T,
    ) -> Result<T, TransportError> {
        let mut state = self.state.lock().map_err(|_| TransportError::Authority)?;
        Self::prune(&mut state);
        let entry = state
            .entries
            .values()
            .find(|e| e.context == context)
            .ok_or(TransportError::Authority)?;
        let mut entries = entry
            .recorder
            .entries(context)
            .map_err(|_| TransportError::Authority)?;
        Ok(inspect(&mut entries))
    }
}
pub(crate) struct RecordingSession {
    owner: Arc<Diagnostics>,
    id: u64,
}
impl RecordingSession {
    pub fn record(
        &self,
        direction: Direction,
        kind: p::Kind,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        // Exclude both handshake packets, even though ServerHello is public.
        if matches!(kind, p::Kind::ClientHello | p::Kind::ServerHello) {
            return Ok(());
        }
        let mut state = self
            .owner
            .state
            .lock()
            .map_err(|_| TransportError::Authority)?;
        Diagnostics::prune(&mut state);
        let entry = state
            .entries
            .get_mut(&self.id)
            .ok_or(TransportError::Authentication)?;
        entry
            .recorder
            .record(entry.context, direction, kind as u32, bytes)
            .map_err(|_| TransportError::Limit)?;
        Ok(())
    }
}
impl Drop for RecordingSession {
    fn drop(&mut self) {
        // A panicking local inspection callback must not strand private bytes
        // or their view charge. Other operations still fail closed on poison.
        let mut state = self
            .owner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.entries.remove(&self.id);
    }
}
