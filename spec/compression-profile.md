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
