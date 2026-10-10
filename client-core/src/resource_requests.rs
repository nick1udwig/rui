//! Shared bounded resource-request cursor. The replica owns charged digest sets;
//! platform queues bound each page and retain its charged encoded-byte lifetime.
use rui_protocol_core::receiver::NeededResources;
use rui_wire::{proto as p, WireError};
#[derive(Default)]
pub struct ResourceRequests {
    needed: Option<NeededResources>,
    requested: usize,
    announce_evictions: bool,
}
#[derive(Clone, Copy)]
pub struct Page {
    start: usize,
    count: usize,
    pub announce_evictions: bool,
}
impl ResourceRequests {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn begin(&mut self, needed: NeededResources, cache_loss: bool) -> Result<(), WireError> {
        if needed.len() > 512 {
            return Err(WireError::Limit);
        }
        self.needed = Some(needed);
        self.requested = 0;
        self.announce_evictions = cache_loss;
        Ok(())
    }
    pub fn contains(&self, digest: &[u8; 32]) -> bool {
        self.needed.as_ref().is_some_and(|n| n.contains(digest))
    }
    pub fn active(&self) -> bool {
        self.needed.is_some()
    }
    pub fn page(&self, envelope: usize) -> Result<Option<Page>, WireError> {
        let Some(needed) = &self.needed else {
            return Ok(None);
        };
        if self.requested >= needed.len() {
            return Ok(None);
        }
        let maximum = envelope.saturating_sub(256) / 40;
        if maximum == 0 {
            return Err(WireError::Limit);
        }
        Ok(Some(Page {
            start: self.requested,
            count: (needed.len() - self.requested).min(maximum),
            announce_evictions: self.announce_evictions,
        }))
    }
    pub fn payload_len(page: Page) -> usize {
        page.count * 34
    }
    pub fn payload(&self, page: Page) -> p::ResourceIds {
        p::ResourceIds {
            digests: self
                .needed
                .as_ref()
                .expect("active resource page")
                .iter()
                .skip(page.start)
                .take(page.count)
                .map(|d| d.to_vec())
                .collect(),
        }
    }
    pub fn queued(&mut self, page: Page) {
        self.requested += page.count;
    }
}
