//! Negotiated publication compression for native senders.
use crate::{codec, proto as p, TransportError};
use prost::Message;
use rui_wire::compression::History;

pub fn compress_with_history(
    packet: &mut p::Packet,
    history: &mut History,
) -> Result<(), TransportError> {
    use std::io::Write;
    if packet.kind == p::Kind::SnapshotBegin as i32
        || packet.kind == p::Kind::AbortPublication as i32
    {
        history.clear();
    }
    if packet.kind != p::Kind::PublicationChunk as i32 {
        return Ok(());
    }
    let mut chunk: p::PublicationChunk = codec::decode(&packet.payload, 1024 * 1024)?;
    let original = chunk.data.clone();
    compress_publication_chunk(packet)?;
    if !original.is_empty()
        && original.len() <= rui_wire::compression::MAX_CHUNK
        && !history.bytes().is_empty()
        && !history.bytes().starts_with(&[0x37, 0xa4, 0x30, 0xec])
    {
        let mut encoder = zstd::stream::Encoder::with_dictionary(Vec::new(), 3, history.bytes())
            .map_err(|_| TransportError::Protocol)?;
        encoder
            .window_log(18)
            .map_err(|_| TransportError::Protocol)?;
        encoder
            .set_pledged_src_size(Some(original.len() as u64))
            .map_err(|_| TransportError::Protocol)?;
        encoder
            .write_all(&original)
            .map_err(|_| TransportError::Protocol)?;
        let mut encoded = history.digest().to_vec();
        encoded.extend(encoder.finish().map_err(|_| TransportError::Protocol)?);
        chunk.data = encoded;
        let candidate = chunk.encode_to_vec();
        if candidate.len() < packet.payload.len() {
            packet.kind = p::Kind::ZstdHistoryChunk as i32;
            packet.payload = candidate;
        }
    }
    history.update(&original);
    Ok(())
}

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
    fn history_is_bounded_exact_and_rejects_the_wrong_base() {
        let mut sender = History::default();
        let mut receiver = History::default();
        let mut encoded_history = 0;
        for i in 0u64..40 {
            let original: Vec<u8> = (0u64..7000)
                .flat_map(|j| (j ^ (i % 2)).to_le_bytes())
                .collect();
            let mut packet = codec::wrap(
                p::Kind::PublicationChunk,
                &p::PublicationChunk {
                    publication_id: vec![1; 16],
                    index: i as u32,
                    data: original.clone(),
                },
                None,
            );
            compress_with_history(&mut packet, &mut sender).unwrap();
            let chunk: p::PublicationChunk = codec::decode(&packet.payload, 1024 * 1024).unwrap();
            let decoded = match p::Kind::try_from(packet.kind).unwrap() {
                p::Kind::ZstdHistoryChunk => {
                    encoded_history += 1;
                    assert!(History::default().decode(&chunk.data, 65536).is_err());
                    receiver.decode(&chunk.data, 65536).unwrap()
                }
                p::Kind::ZstdPublicationChunk => {
                    rui_wire::compression::decode(&chunk.data, 65536).unwrap()
                }
                _ => chunk.data,
            };
            assert_eq!(decoded, original);
            receiver.update(&decoded);
            assert_eq!(receiver.digest(), sender.digest());
            assert!(receiver.bytes().len() <= rui_wire::compression::HISTORY_BYTES);
        }
        assert!(encoded_history > 30);
        let mut packet = codec::wrap(
            p::Kind::SnapshotBegin,
            &p::PublicationBegin::default(),
            None,
        );
        compress_with_history(&mut packet, &mut sender).unwrap();
        assert!(sender.bytes().is_empty());
    }
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
