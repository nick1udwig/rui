# Portable client core

This crate has no GTK, Cairo, socket, TLS or platform clock dependencies. It shares the native client's interaction, composition, typing, command queue and source-control builders, while adding a synchronous session actor for WASM and other platform integrations.

`session::Session` takes authenticated ordered binary frames, explicit monotonic times and presentation events. It negotiates existing RUI profiles/limits, delegates exact-byte publications and verified resources to `ViewReceiver`, queues replies with bounded ownership, manages control requests, and retains exact pending commands across disconnects. The caller owns transport authentication, deadlines and presentation truth. A resumed connection still opens the view. A changed epoch retires old commands; same-scope reconnect requires identical negotiated limits and profiles.

The native desktop preserves its module API through reexports. Its networking actor remains `NetworkClient`; this extraction does not claim that the two platform connection actors have already been consolidated. Both use `rui-wire`'s codec and shared guarded interaction/command behavior. [The behavior matrix](../docs/client-behavior-matrix.md) tracks migration gaps and cancellation ownership.

```sh
cargo test --locked -p rui-client-core
cargo check --locked -p rui-client-wasm --target wasm32-unknown-unknown
```
