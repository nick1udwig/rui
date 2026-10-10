# Portable wire layer

`rui-wire` owns the generated session schema, recursively bounded codec/preflight,
portable `WireError` and redacted/zeroizing command/credential containers. It
depends on protocol-core and has no socket, GUI, TLS or platform clock dependency.
The codec consumes exact bytes; it never rewrites commands or publication digests.
Transport reexports `codec`, `proto` and the compatibility `TransportError` alias.

Builds use the same locked prost generator and unchanged `spec/session.proto`.
Native transport depends on client-core and wire; client-core depends on wire and
protocol-core. WASM no longer builds the transport package merely for its codec.
Native socket/sink cancellation ownership stays in transport until the common
actor has native parity and its cancellation regressions pass through that path.

```sh
cargo test -p rui-wire -p rui-client-core --locked
cargo check -p rui-client-wasm --target wasm32-unknown-unknown --locked
```
