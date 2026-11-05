//! Independently decodable publication chunks with bounded output and history.
use crate::WireError;
use std::io::Read;

pub const MAX_CHUNK: usize = 64 * 1024;
pub const DECODE_MEMORY: usize = 2 * 1024 * 1024;

pub fn decode(bytes: &[u8], limit: usize) -> Result<Vec<u8>, WireError> {
    let (frame, _) = ruzstd::frame::read_frame_header(bytes).map_err(|_| WireError::Protocol)?;
    let size = usize::try_from(frame.header.frame_content_size()).map_err(|_| WireError::Limit)?;
    if size == 0
        || size > limit.min(MAX_CHUNK)
        || frame
            .header
            .window_size()
            .map_err(|_| WireError::Protocol)?
            > MAX_CHUNK as u64
        || frame.header.dictionary_id().is_some()
    {
        return Err(WireError::Limit);
    }
    let mut decoder = ruzstd::StreamingDecoder::new(bytes).map_err(|_| WireError::Protocol)?;
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
