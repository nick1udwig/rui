//! Independently decodable publication chunks with bounded output and history.
use crate::WireError;
use sha2::{Digest, Sha256};
use std::io::Read;

pub const MAX_CHUNK: usize = 64 * 1024;
pub const DECODE_MEMORY: usize = 2 * 1024 * 1024;
pub const HISTORY_BYTES: usize = 256 * 1024;
pub const HISTORY_MEMORY: usize = HISTORY_BYTES + 1024;

#[derive(Default)]
pub struct History {
    bytes: Vec<u8>,
}
impl History {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn clear(&mut self) {
        self.bytes.clear();
    }
    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(&self.bytes).into()
    }
    pub fn update(&mut self, bytes: &[u8]) {
        if self.bytes.capacity() < HISTORY_BYTES {
            self.bytes.reserve_exact(HISTORY_BYTES - self.bytes.len());
        }
        if bytes.len() >= HISTORY_BYTES {
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&bytes[bytes.len() - HISTORY_BYTES..]);
        } else {
            let discard = (self.bytes.len() + bytes.len()).saturating_sub(HISTORY_BYTES);
            self.bytes.drain(..discard);
            self.bytes.extend_from_slice(bytes);
        }
    }
    pub fn decode(&self, bytes: &[u8], limit: usize) -> Result<Vec<u8>, WireError> {
        if bytes.len() < 32 || bytes[..32] != self.digest() {
            return Err(WireError::Protocol);
        }
        decode_with_history(&bytes[32..], limit, Some(self))
    }
}

pub fn decode(bytes: &[u8], limit: usize) -> Result<Vec<u8>, WireError> {
    decode_with_history(bytes, limit, None)
}
fn decode_with_history(
    bytes: &[u8],
    limit: usize,
    history: Option<&History>,
) -> Result<Vec<u8>, WireError> {
    let (frame, _) = ruzstd::frame::read_frame_header(bytes).map_err(|_| WireError::Protocol)?;
    let size = usize::try_from(frame.header.frame_content_size()).map_err(|_| WireError::Limit)?;
    if size == 0
        || size > limit.min(MAX_CHUNK)
        || frame
            .header
            .window_size()
            .map_err(|_| WireError::Protocol)?
            > if history.is_some() {
                HISTORY_BYTES
            } else {
                MAX_CHUNK
            } as u64
        || frame.header.dictionary_id().is_some()
    {
        return Err(WireError::Limit);
    }
    let mut decoder = ruzstd::StreamingDecoder::new(bytes).map_err(|_| WireError::Protocol)?;
    if let Some(history) = history {
        let scratch = ruzstd::decoding::scratch::DecoderScratch::new(0);
        decoder
            .decoder
            .add_dict(ruzstd::decoding::dictionary::Dictionary {
                id: 1,
                fse: scratch.fse,
                huf: scratch.huf,
                dict_content: history.bytes.clone(),
                offset_hist: [1, 4, 8],
            })
            .map_err(|_| WireError::Protocol)?;
        decoder
            .decoder
            .force_dict(1)
            .map_err(|_| WireError::Protocol)?;
    }
    let mut output = vec![0; size];
    decoder
        .read_exact(&mut output)
        .map_err(|_| WireError::Protocol)?;
    let mut extra = [0];
    if decoder.read(&mut extra).map_err(|_| WireError::Protocol)? != 0
        || !decoder.decoder.is_finished()
        || !decoder.get_ref().is_empty()
    {
        return Err(WireError::Protocol);
    }
    Ok(output)
}
