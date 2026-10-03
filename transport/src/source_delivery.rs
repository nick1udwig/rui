//! One bounded producer publication and its immutable resource replies. The
//! publication frame stays owned through APPLIED, including while the host
//! advances or evicts its independent recovery history.
use crate::server::Outgoing;
use crate::{codec, proto as p, TransportError};
use rui_host_broker::{AuthorizedResource, ControlledHost};
use rui_protocol_core::{
    commands::Id,
    session::{NegotiatedSession, SharedPayload, ViewMemoryReservation},
};
use std::{
    collections::{BTreeSet, VecDeque},
    time::Instant,
};
const MAX_RESOURCES: usize = 512;
pub(crate) struct Delivery {
    pub publication: Option<Outgoing>,
    last_applied: Option<(u64, SharedPayload)>,
    resources: VecDeque<ResourceTransfer>,
    pending: BTreeSet<[u8; 32]>,
    completed: BTreeSet<[u8; 32]>,
    evicted: BTreeSet<[u8; 32]>,
    pub started: Option<Instant>,
    _memory: ViewMemoryReservation,
}
impl Delivery {
    pub fn new(host: &ControlledHost, session: &NegotiatedSession) -> Result<Self, TransportError> {
        // Bounded digest sets, spare queue capacity and immutable handles. Resource
        // bodies/frame attachments already carry their independent view charge.
        let memory = host
            .reserve_view_memory(&session.client, session.view_id, MAX_RESOURCES * 1024)
            .map_err(|_| TransportError::Limit)?;
        Ok(Self {
            publication: None,
            last_applied: None,
            resources: VecDeque::new(),
            pending: BTreeSet::new(),
            completed: BTreeSet::new(),
            evicted: BTreeSet::new(),
            started: None,
            _memory: memory,
        })
    }
    pub fn install(&mut self, publication: Outgoing) {
        debug_assert!(self.publication.is_none());
        self.publication = Some(publication);
        self.started = Some(Instant::now());
    }
    pub fn need(
        &mut self,
        host: &ControlledHost,
        session: &NegotiatedSession,
        digests: p::ResourceIds,
        chunk: usize,
    ) -> Result<(), TransportError> {
        if digests.digests.is_empty() || digests.digests.len() > MAX_RESOURCES {
            return Err(TransportError::Limit);
        }
        let mut requested = BTreeSet::new();
        let mut transfers = VecDeque::new();
        for raw in &digests.digests {
            let digest = codec::digest(raw)?;
            if !requested.insert(digest) {
                return Err(TransportError::Protocol);
            }
            if self.pending.contains(&digest) {
                continue;
            }
            if self.pending.len() + transfers.len() >= MAX_RESOURCES {
                return Err(TransportError::Limit);
            }
            // Exact in-flight or last APPLIED closure, not a known hash or even
            // arbitrary retained history. Host rechecks current scope itself.
            let selected = self
                .publication
                .as_ref()
                .and_then(|publication| {
                    host.publication_resource(
                        &session.client,
                        session.view_id,
                        &publication.frame,
                        digest,
                    )
                    .ok()
                })
                .or_else(|| {
                    self.last_applied.as_ref().and_then(|(_, frame)| {
                        host.publication_resource(&session.client, session.view_id, frame, digest)
                            .ok()
                    })
                })
                .ok_or(TransportError::Authority)?;
            let transfer = ResourceTransfer::new(selected, chunk)?;
            transfers.push_back(transfer);
        }
        self.pending.extend(requested);
        self.resources.extend(transfers);
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
        Ok(())
    }
    pub fn evicted(
        &mut self,
        host: &ControlledHost,
        session: &NegotiatedSession,
        ids: &p::ResourceIds,
    ) -> Result<(), TransportError> {
        if ids.digests.is_empty() || ids.digests.len() > MAX_RESOURCES {
            return Err(TransportError::Limit);
        }
        let mut unique = BTreeSet::new();
        for raw in &ids.digests {
            let digest = codec::digest(raw)?;
            if !unique.insert(digest) {
                return Err(TransportError::Protocol);
            }
            let current = self.publication.as_ref().is_some_and(|publication| {
                host.publication_resource(
                    &session.client,
                    session.view_id,
                    &publication.frame,
                    digest,
                )
                .is_ok()
            });
            let live = self.last_applied.as_ref().is_some_and(|(_, frame)| {
                host.publication_resource(&session.client, session.view_id, frame, digest)
                    .is_ok()
            });
            if !current && !live {
                return Err(TransportError::Authority);
            }
        }
        if self.evicted.len() + unique.iter().filter(|d| !self.evicted.contains(*d)).count()
            > MAX_RESOURCES
        {
            return Err(TransportError::Limit);
        }
        self.evicted.extend(unique);
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
        Ok(())
    }
    pub fn available(&mut self, ids: p::ResourceIds) -> Result<(), TransportError> {
        if ids.digests.is_empty() || ids.digests.len() > MAX_RESOURCES {
            return Err(TransportError::Limit);
        }
        let mut unique = BTreeSet::new();
        for raw in ids.digests {
            let digest = codec::digest(&raw)?;
            if !unique.insert(digest) || !self.completed.contains(&digest) {
                return Err(TransportError::Protocol);
            }
        }
        for digest in unique {
            self.completed.remove(&digest);
            self.pending.remove(&digest);
            self.evicted.remove(&digest);
        }
        if self.publication.is_none() && self.pending.is_empty() && self.evicted.is_empty() {
            self.started = None;
        }
        Ok(())
    }
    pub fn applied(&mut self, revision: u64) -> Result<(), TransportError> {
        if self
            .publication
            .as_ref()
            .is_some_and(|p| p.begin.revision == revision)
        {
            let publication = self.publication.as_ref().unwrap();
            if publication.stage != 2 || !self.pending.is_empty() || !self.evicted.is_empty() {
                return Err(TransportError::Protocol);
            }
            let publication = self.publication.take().unwrap();
            self.last_applied = Some((revision, publication.frame));
            self.started = None;
            self.completed.clear();
            self.resources.clear();
            return Ok(());
        }
        if self
            .last_applied
            .as_ref()
            .is_some_and(|(r, _)| *r == revision)
        {
            if !self.pending.is_empty()
                || !self.evicted.is_empty()
                || !self.completed.is_empty()
                || self.resources.iter().any(|r| r.stage != 3)
            {
                return Err(TransportError::Protocol);
            }
            return Ok(());
        }
        Err(TransportError::Protocol)
    }
    pub fn applied_frame(&self, revision: u64) -> Result<&SharedPayload, TransportError> {
        self.last_applied
            .as_ref()
            .filter(|(r, _)| *r == revision)
            .map(|(_, frame)| frame)
            .ok_or(TransportError::Protocol)
    }
    pub fn next(
        &mut self,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<Option<p::Packet>, TransportError> {
        while let Some(resource) = self.resources.front_mut() {
            if let Some(packet) = resource.next(binding)? {
                if resource.stage == 3 {
                    self.completed.insert(resource.resource.offer().digest);
                }
                return Ok(Some(packet));
            }
            self.resources.pop_front();
        }
        if let Some(publication) = &mut self.publication {
            return publication.next(binding);
        }
        Ok(None)
    }
    pub fn can_start(&self) -> bool {
        self.publication.is_none() && self.pending.is_empty() && self.evicted.is_empty()
    }
}
struct ResourceTransfer {
    resource: AuthorizedResource,
    stage: u32,
    index: u32,
    chunks: u32,
    chunk: usize,
}
impl ResourceTransfer {
    fn new(resource: AuthorizedResource, chunk: usize) -> Result<Self, TransportError> {
        if chunk == 0 {
            return Err(TransportError::Limit);
        }
        let chunks = u32::try_from(resource.bytes().len().div_ceil(chunk))
            .map_err(|_| TransportError::Limit)?;
        if chunks == 0 || chunks > 4096 {
            return Err(TransportError::Limit);
        }
        Ok(Self {
            resource,
            stage: 0,
            index: 0,
            chunks,
            chunk,
        })
    }
    fn next(&mut self, binding: Option<(Id, Id, Id)>) -> Result<Option<p::Packet>, TransportError> {
        let offer = self.resource.offer();
        if self.stage == 0 {
            self.stage = 1;
            return Ok(Some(codec::wrap(
                p::Kind::ResourceOffer,
                &p::ResourceOffer {
                    digest: offer.digest.to_vec(),
                    kind: offer.kind as u32,
                    format_version: offer.version,
                    total_bytes: offer.total_bytes,
                    chunk_count: self.chunks,
                    provenance: offer.provenance as u32,
                },
                binding,
            )));
        }
        if self.index < self.chunks {
            let start = self.index as usize * self.chunk;
            let end = (start + self.chunk).min(self.resource.bytes().len());
            let packet = codec::wrap(
                p::Kind::ResourceChunk,
                &p::ResourceChunk {
                    digest: offer.digest.to_vec(),
                    index: self.index,
                    data: self.resource.bytes()[start..end].to_vec(),
                },
                binding,
            );
            self.index += 1;
            return Ok(Some(packet));
        }
        if self.stage == 1 {
            self.stage = 3;
            return Ok(Some(codec::wrap(
                p::Kind::ResourceComplete,
                &p::ResourceComplete {
                    digest: offer.digest.to_vec(),
                },
                binding,
            )));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_host_broker::{
        HostConfig, SourceFontError, SourceFontProvider, SourceGlyph, SourceOutline,
        SourceShapedText,
    };
    use rui_protocol_core::{resources::*, session::*};
    use std::sync::Arc;
    struct OneOutline;
    impl SourceFontProvider for OneOutline {
        fn shape(&self, _text: &str, size: f64) -> Result<SourceShapedText, SourceFontError> {
            let mut bytes = b"RVP1".to_vec();
            bytes.extend_from_slice(&3u32.to_be_bytes());
            for (tag, x, y) in [(1, 0.0f64, 0.0f64), (2, 1.0, 1.0)] {
                bytes.push(tag);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
            }
            bytes.push(5);
            let digest = resource_digest(
                ResourceKind::GlyphOutline,
                1,
                ResourceProvenance::FontOutline,
                &bytes,
            );
            let outline = SourceOutline::new(
                ResourceOffer {
                    kind: ResourceKind::GlyphOutline,
                    version: 1,
                    provenance: ResourceProvenance::FontOutline,
                    total_bytes: bytes.len() as u64,
                    digest,
                },
                bytes,
            )?;
            Ok(SourceShapedText {
                glyphs: vec![SourceGlyph {
                    resource: digest,
                    size,
                    position: [0.0, 12.0],
                }],
                resources: vec![outline],
            })
        }
    }
    #[test]
    fn cache_loss_requires_completion_and_available_before_reapplied() {
        let mut host = ControlledHost::new_corpus_with_fonts(
            HostConfig::default(),
            "PRIVATE".into(),
            Arc::new(OneOutline),
        )
        .unwrap();
        let mut profiles = SessionConfig::default().supported_profiles;
        profiles.insert("glyph.outline-single.v1".into());
        let session = host
            .negotiate(
                VerifiedAuthorization {
                    principal: "alice".into(),
                    policy_version: 1,
                    permissions: Permissions {
                        can_view: true,
                        can_control: false,
                        actions: Default::default(),
                    },
                },
                ClientHello {
                    client_instance_id: [3; 16],
                    supported_versions: [1].into_iter().collect(),
                    supported_profiles: profiles,
                    required_profiles: SessionConfig::default().required_profiles,
                    receiver_limits: ReceiverLimits::default(),
                    resume: None,
                },
                0,
            )
            .unwrap();
        let baseline = host
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap();
        let mut delivery = Delivery::new(&host, &session).unwrap();
        let frame = host.checkpoint(&session.client, session.view_id).unwrap();
        let digest = host
            .source_capture(&session.client, session.view_id)
            .unwrap()
            .resources()
            .next()
            .unwrap()
            .0
            .digest;
        delivery.install(Outgoing::new(frame, 512, session.limits.snapshot_bytes).unwrap());
        while delivery.next(None).unwrap().is_some() {}
        delivery.applied(1).unwrap();
        let ids = || p::ResourceIds {
            digests: vec![digest.to_vec()],
        };
        for invalid in [
            vec![vec![8; 31]],
            vec![vec![8; 32]],
            vec![digest.to_vec(), digest.to_vec()],
        ] {
            assert!(delivery
                .evicted(&host, &session, &p::ResourceIds { digests: invalid })
                .is_err());
            assert!(delivery.evicted.is_empty());
            assert!(delivery.started.is_none());
            delivery.applied(1).unwrap();
        }
        delivery.evicted(&host, &session, &ids()).unwrap();
        assert!(delivery.started.is_some());
        assert!(!delivery.can_start());
        // Declared loss itself blocks re-APPLIED, even before NeedResources.
        assert!(delivery.applied(1).is_err());
        delivery.need(&host, &session, ids(), 16).unwrap();
        assert!(delivery.applied(1).is_err());
        while delivery.next(None).unwrap().is_some() {}
        assert!(delivery.applied(1).is_err());
        delivery.available(ids()).unwrap();
        delivery.applied(1).unwrap();
        assert!(delivery.can_start());
        assert!(delivery.started.is_none());
        let retained = host
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap();
        assert!(retained <= session.limits.view_memory_bytes);
        drop(delivery);
        assert!(
            host.retained_memory_bytes(&session.client, session.view_id)
                .unwrap()
                < retained
        );
        assert!(
            host.retained_memory_bytes(&session.client, session.view_id)
                .unwrap()
                >= baseline
        );
    }
}
