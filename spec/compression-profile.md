# Publication chunk compression

`transport.zstd-chunks.v1` is an optional session profile.
Clients advertise support and hosts select it through the existing profile intersection.
An unnegotiated compressed chunk is a protocol error.
Older peers continue to exchange ordinary publication chunks.

`ZSTD_PUBLICATION_CHUNK` uses the `PublicationChunk` schema with one complete Zstandard frame in `data`.
Each frame has a known, nonzero decoded length and a maximum 64 KiB decoded size and history window.
The negotiated chunk limit also applies to the decoded length.
Dictionaries, concatenated frames, trailing bytes, and truncated frames are rejected.
Each chunk is independent, including across publication, view, and connection boundaries.
Hosts use level 3 and keep the ordinary encoding whenever compression would increase the data length.
Chunks larger than 64 KiB retain their ordinary encoding.

Publication IDs, chunk indexes, begin/commit lengths, hashes, and item counts describe the original decoded bytes.
The existing atomic receiver validates those bytes and their resource closure before committing.
Compression does not change acknowledgement or input authority rules.
Snapshots, reconnects, aborts, and duplicate chunks require no additional compression state.

The shared Rust receiver provides decoding for native and WASM clients.
Before decoding it checks the frame header and reserves 2 MiB of codec working memory in the view budget.
The host reserves 4 MiB while compressing a chunk for encoder working memory.
These reservations are in addition to existing packet and publication accounting.
WebSocket extension compression remains disabled; this profile compresses only publication data after authentication.
Resources and commands retain their existing encoding.

## Bounded history

`transport.zstd-history.v1` optionally extends the chunk profile.
Both profiles must be selected to enable history.
The host and receiver retain the last 256 KiB of decoded publication chunk data in delivery order, including ordinary and independently compressed chunks.
Each `ZSTD_HISTORY_CHUNK` contains a 32-byte SHA-256 digest of that history followed by a complete Zstandard frame using the history as a raw dictionary.
The frame has no dictionary ID, a known decoded length of at most 64 KiB, and a window of at most 256 KiB.
The sender chooses history only when the entire candidate payload is smaller than both the ordinary and independent encodings.

History belongs to one authenticated connection and view.
Snapshot begin and publication abort clear it; disconnect and new capability installation discard it.
A reconnect starts empty, even when the logical replica resumes from an existing revision.
The history digest detects an incorrect base before decompression.
This extension requires ordered, exactly-once chunk delivery from its reliable transport; replaying an old compressed packet against a later history fails closed.
Logical resume sends publications through a fresh encoder rather than reusing old compressed packets.
The independent chunk profile retains its independent replay behavior.

Each side reserves 256 KiB plus metadata for retained history.
History decoding additionally reserves 512 KiB of working memory beyond the independent decoder reservation.
The encoder reservation remains 4 MiB while processing a chunk.
The history digest adds 32 bytes to each history-compressed chunk and is included in size selection and measurements.
No history is shared between principals, views, or connections.
