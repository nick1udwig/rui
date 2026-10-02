//! Pinned vector assets; never obtain cursor images from the user's platform.
use crate::{ResourceProvider, ResourceView};
use rui_protocol_core::{
    model::CursorShape,
    resources::{resource_digest, ResourceKind, ResourceOffer, ResourceProvenance},
};
pub(crate) struct Asset {
    offer: ResourceOffer,
    body: &'static [u8],
}
impl Asset {
    pub fn new(shape: CursorShape) -> Option<Self> {
        let body: &'static [u8] = match shape {
            CursorShape::Default => include_bytes!("../../fixtures/render/cursors/default.rvp"),
            CursorShape::Pointer => include_bytes!("../../fixtures/render/cursors/pointer.rvp"),
            CursorShape::Text => include_bytes!("../../fixtures/render/cursors/text.rvp"),
            CursorShape::Hidden => return None,
        };
        Some(Self {
            offer: ResourceOffer {
                kind: ResourceKind::VectorPath,
                version: 1,
                total_bytes: body.len() as u64,
                provenance: ResourceProvenance::AuthoredVector,
                digest: resource_digest(
                    ResourceKind::VectorPath,
                    1,
                    ResourceProvenance::AuthoredVector,
                    body,
                ),
            },
            body,
        })
    }
    pub fn digest(&self) -> [u8; 32] {
        self.offer.digest
    }
}
impl ResourceProvider for Asset {
    fn get(&self, digest: &[u8; 32]) -> Option<ResourceView<'_>> {
        (*digest == self.offer.digest).then_some(ResourceView {
            offer: &self.offer,
            bytes: self.body,
        })
    }
}
