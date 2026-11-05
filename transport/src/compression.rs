//! Negotiated publication compression for native senders.
use crate::{codec, proto as p, TransportError};
use prost::Message;

pub fn compress_publication_chunk(packet: &mut p::Packet) -> Result<(), TransportError> {
    use std::io::Write;
    if packet.kind != p::Kind::PublicationChunk as i32 {
        return Ok(());
    }
    let mut chunk: p::PublicationChunk = codec::decode(&packet.payload, 1024 * 1024)?;
    if chunk.data.len() > rui_wire::compression::MAX_CHUNK {
        return Ok(());
    }
    let mut encoder =
        zstd::stream::Encoder::new(Vec::new(), 3).map_err(|_| TransportError::Protocol)?;
    encoder
        .window_log(16)
        .map_err(|_| TransportError::Protocol)?;
    encoder
        .set_pledged_src_size(Some(chunk.data.len() as u64))
        .map_err(|_| TransportError::Protocol)?;
    encoder
        .write_all(&chunk.data)
        .map_err(|_| TransportError::Protocol)?;
    let encoded = encoder.finish().map_err(|_| TransportError::Protocol)?;
    if encoded.len() < chunk.data.len() {
        chunk.data = encoded;
        packet.kind = p::Kind::ZstdPublicationChunk as i32;
        packet.payload = chunk.encode_to_vec();
    }
    Ok(())
}

#[cfg(test)]
mod compression_tests {
    use super::*;
    #[test]
    fn compressed_chunks_preserve_bytes_and_enforce_limits() {
        for size in [1, 1000, 65536] {
            let original = vec![42; size];
            let mut packet = codec::wrap(
                p::Kind::PublicationChunk,
                &p::PublicationChunk {
                    publication_id: vec![1; 16],
                    index: 3,
                    data: original.clone(),
                },
                None,
            );
            compress_publication_chunk(&mut packet).unwrap();
            let chunk: p::PublicationChunk = codec::decode(&packet.payload, 1024 * 1024).unwrap();
            assert_eq!(chunk.index, 3);
            assert_eq!(chunk.publication_id, vec![1; 16]);
            if size == 1 {
                assert_eq!(packet.kind, p::Kind::PublicationChunk as i32);
                continue;
            }
            assert_eq!(packet.kind, p::Kind::ZstdPublicationChunk as i32);
            assert!(chunk.data.len() < size);
            assert_eq!(
                rui_wire::compression::decode(&chunk.data, size).unwrap(),
                original
            );
            assert!(rui_wire::compression::decode(&chunk.data, size - 1).is_err());
            for end in 0..chunk.data.len() {
                assert!(rui_wire::compression::decode(&chunk.data[..end], size).is_err());
            }
            let mut trailing = chunk.data.clone();
            trailing.push(0);
            assert!(rui_wire::compression::decode(&trailing, size).is_err());
        }
        let oversized = zstd::bulk::compress(&vec![42; 65537], 3).unwrap();
        assert!(rui_wire::compression::decode(&oversized, 1000000).is_err());
    }
}
