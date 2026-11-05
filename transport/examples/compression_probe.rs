//! Replay actual publication compression through portable session packets.
use prost::Message;
use rui_protocol_core::{
    publication::publication_digest, session::ReceiverLimits, wire::proto as core,
};
use rui_transport::{
    codec,
    compression::{compress_publication_chunk, compress_with_history},
    proto as p,
};
use std::{
    fs::File,
    io::{Read, Write},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 || args.len() > 4 {
        return Err("usage: compression_probe FRAMED_CORE_ENVELOPES OUTPUT_PACKETS".into());
    }
    let use_history = args.get(3).is_some_and(|v| v == "history");
    let mut history = rui_wire::compression::History::default();
    let mut input = File::open(&args[1])?;
    let mut output = File::create(&args[2])?;
    let binding = Some(([1; 16], [2; 16], [4; 16]));
    let mut plain_bytes = 0;
    let mut encoded_bytes = 0;
    let mut chunks = 0;
    let mut compressed_chunks = 0;
    let mut send = |packet: p::Packet| -> Result<(), Box<dyn std::error::Error>> {
        let bytes = packet.encode_to_vec();
        output.write_all(&(bytes.len() as u32).to_be_bytes())?;
        output.write_all(&bytes)?;
        Ok(())
    };
    send(codec::wrap(
        p::Kind::ServerHello,
        &p::ServerHello {
            selected_version: 1,
            selected_profiles: [
                "mirror2d.core.v1",
                "semantics.core.v1",
                "content.no-raster.v1",
                "glyph.outline-single.v1",
                "transport.zstd-chunks.v1",
                "transport.zstd-history.v1",
            ]
            .map(str::to_owned)
            .to_vec(),
            negotiated_limits: Some(codec::wire_limits(&ReceiverLimits::default())),
            permissions: Some(p::Permissions {
                can_view: true,
                ..Default::default()
            }),
            principal_id: vec![3; 16],
        },
        binding,
    ))?;
    let mut frames = 0u64;
    loop {
        let mut length = [0; 4];
        if input.read(&mut length[..1])? == 0 {
            break;
        }
        input.read_exact(&mut length[1..])?;
        let length = u32::from_be_bytes(length) as usize;
        if length > 1024 * 1024 {
            return Err("oversized envelope".into());
        }
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        let envelope = core::Envelope::decode(bytes.as_slice())?;
        let (kind, base, revision, count) = if envelope.kind == 1 {
            let s = core::Snapshot::decode(envelope.payload.as_slice())?;
            (p::Kind::SnapshotBegin, 0, s.revision, s.records.len())
        } else {
            let t = core::Transaction::decode(envelope.payload.as_slice())?;
            (
                p::Kind::TransactionBegin,
                t.base_revision,
                t.new_revision,
                t.mutations.len(),
            )
        };
        let mut id = vec![0; 16];
        id[..8].copy_from_slice(&revision.to_be_bytes());
        let begin = p::PublicationBegin {
            publication_id: id.clone(),
            base_revision: base,
            revision,
            total_bytes: envelope.payload.len() as u64,
            chunk_count: envelope.payload.len().div_ceil(65536) as u32,
            item_count: count as u32,
            digest: publication_digest(&envelope.payload).to_vec(),
        };
        let packet = codec::wrap(kind, &begin, binding);
        plain_bytes += packet.encoded_len();
        encoded_bytes += packet.encoded_len();
        send(packet)?;
        for (index, data) in envelope.payload.chunks(65536).enumerate() {
            let mut packet = codec::wrap(
                p::Kind::PublicationChunk,
                &p::PublicationChunk {
                    publication_id: id.clone(),
                    index: index as u32,
                    data: data.to_vec(),
                },
                binding,
            );
            plain_bytes += packet.encoded_len();
            if use_history {
                compress_with_history(&mut packet, &mut history)?;
            } else {
                compress_publication_chunk(&mut packet)?;
            }
            encoded_bytes += packet.encoded_len();
            chunks += 1;
            if matches!(
                p::Kind::try_from(packet.kind)?,
                p::Kind::ZstdPublicationChunk | p::Kind::ZstdHistoryChunk
            ) {
                compressed_chunks += 1;
            }
            send(packet)?;
        }
        let packet = codec::wrap(
            p::Kind::PublicationCommit,
            &p::PublicationCommit {
                publication_id: id,
                total_bytes: begin.total_bytes,
                chunk_count: begin.chunk_count,
                item_count: begin.item_count,
                digest: begin.digest,
            },
            binding,
        );
        plain_bytes += packet.encoded_len();
        encoded_bytes += packet.encoded_len();
        send(packet)?;
        frames += 1;
    }
    println!(
        "{}",
        serde_json::json!({"frames":frames,"plain_publication_packet_bytes":plain_bytes,"compressed_publication_packet_bytes":encoded_bytes,"chunks":chunks,"compressed_chunks":compressed_chunks,"scope":"publication begin/chunk/commit packets; excludes handshake, acknowledgements, TLS and WebSocket framing"})
    );
    Ok(())
}
