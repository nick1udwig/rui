# Core resource and publication formats

This implementation accepts only version 1 `VectorPath` (kind 1) with
`AuthoredVector` provenance (1), or `GlyphOutline` (kind 2) with `FontOutline`
provenance (2). Unknown kinds, versions, provenance values and combinations
fail closed. The trusted producer must establish provenance before the broker
authorizes an offer. A structural decoder cannot prove that a malicious
producer did not derive geometry from screenshot pixels. This library does not
provide a raster asset profile or a font file decoder.

The SHA-256 input is exactly the following concatenation:

| Field | Encoding |
|---|---|
| Domain | ASCII `RUI/0.1/resource` followed by one zero byte |
| Kind | unsigned 32-bit big-endian |
| Resource format version | unsigned 32-bit big-endian |
| Exact body length | unsigned 64-bit big-endian |
| Provenance | unsigned 32-bit big-endian |
| Body | exact received bytes |

The provenance field is an explicit implementation format extension to the
RFC's fixed-width resource descriptor. Body bytes are never reserialized to
verify integrity. Cache lookup checks the authenticated session, epoch,
principal and current view grant, including after revocation. A hash does not
create a grant. Cache bodies remain memory-only.

Both approved kinds use the following deliberately limited, flat geometry
format. No opaque resource payloads, external URLs, nested containers,
compression, shaders, image data or executable content are accepted.

| Field | Encoding |
|---|---|
| Magic | four ASCII bytes `RVP1` |
| Command count | unsigned 32-bit big-endian |
| Commands | exactly the declared number, followed by no other bytes |

Each command starts with a one-byte opcode. Every coordinate is finite IEEE
754 binary64, stored big-endian. A point is two coordinates (x, y). Zero uses
the positive-zero encoding. Coordinate magnitude must not exceed the
configured finite, positive bound (default 1,000,000,000 logical units).

| Opcode | Command | Following points |
|---|---|---|
| 1 | MoveTo, starts a contour | destination |
| 2 | LineTo | destination |
| 3 | QuadraticTo | control, destination |
| 4 | CubicTo | control 1, control 2, destination |
| 5 | Close | none |

Drawing commands require an open contour. Close ends it; a subsequent drawing
command requires another MoveTo. Another MoveTo may start a new contour while
the previous contour is open. Open contours are allowed. Empty paths use zero
commands. Paint, fill rules, stroke style, glyph selection and positioning are
scene properties rather than hidden resource behavior. This safe validator
does not claim to render or implement the RFC's full primitive set.

The receiver checks declared body size and resource/chunk/count limits and
reserves the entire body capacity plus bounded bookkeeping in a shared
`MemoryBudget` before allocating a transfer. Chunk indexes are contiguous;
exact duplicates are accepted without adding bytes, while conflicting
duplicates and empty chunks are rejected.
Conflicting duplicates poison their transfer so completion cannot salvage it;
an explicit abort or failed completion releases it before a fresh transfer.
Availability requires full length, exact-byte integrity and safe geometry
validation. An unsuccessful completion
discards its receiving body. Missing resources can already be pinned by a
live, staged or in-flight owner; completing a transfer preserves those pins.
Only unpinned available resources can be evicted, and each eviction returns a
`ResourceEvicted` notification for the caller to send. Closing or revoking the
last authorized owner purges incomplete and complete bodies if no pins remain.

Publication assembly uses SHA-256 of the exact concatenated payload bytes,
without the resource domain header. Begin and Commit must agree on ID-bound
revision metadata, byte length, chunk count, item count and digest. Contiguous
chunks are checked without interpreting record boundaries. A failed Commit
discards staging; Abort and authorization revocation release it. Successful
assembly keeps the bytes charged until the assembled object is dropped.
Committed replica records are owned and validated by the reducer, which must
also check the declared item count against decoded records/mutations and
check the publication revision against its envelope. Complete byte assembly
does not authorize an atomic state commit by itself.

`MemoryBudget` is an allocation ledger, not an operating-system RSS estimator.
It charges declared byte buffer capacity and conservative per-entry
bookkeeping, including unused slots in sparse tree root nodes. Nonempty
availability sets reserve a fixed root allowance plus a per-key charge before
collection and retain that reservation for their entire lifetime. The caller
must share the same ledger with decoded/staged/live replica state and reserve
its allocations before growth; transport buffers and outbound queues need
their own bounded integration. No component in this module fetches external
content or performs network I/O.
