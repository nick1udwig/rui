# RUI

RUI is an experimental, transport-independent remote UI state replication protocol. The host owns application state and layout; clients retain authorized semantic and vector scene state. The strict profile prohibits transmitted raster content.

The design is in [RFC.md](RFC.md). The original [conformance.yaml](conformance.yaml) describes the intended full system; it is not a claim that every case is implemented.

Implementation starts with the **M1 protocol core**. This repository does not yet provide a remote desktop application, a network server, a GTK capture adapter, or a mirror renderer. GTK feasibility (M0) and the controlled graphical vertical slice (M2) remain open gates.

## Development

Install Rust, `protoc`, and Python 3, then run:

```sh
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 conformance/check.py
```

The workspace lockfile pins Rust dependencies. Toolkit types and graphics API handles do not belong in the public protocol.

The Python check independently decodes exact Protobuf golden fixtures and compares Rust/Python logical state after each accepted or rejected update. It exercises seeded replay, equivalent fresh snapshots, and retained values for all nine scene primitives. It needs no third-party Python packages.

## Layout

- `spec/`: initial Protobuf vocabulary and resource encoding details.
- `protocol-core/`: replica model, coordinated receiver, bounded framing/publications/resources, command admission, and session authority.
- `host-broker/`: controlled headless application, two-client projections, and guarded semantic dispatch.
- `conformance/`: independent reference checks and implementation coverage notes.

The implemented vocabulary is deliberately smaller than the proposed RFC. Unsupported required variants must fail explicitly. Passing core tests does not establish visual fidelity, OS input safety, or full RFC conformance.

See [the core integration contract](spec/core-profile.md) for API trust boundaries and [acceptance status](conformance/STATUS.md) for outstanding milestone gates.

The [headless host fixture](spec/host-fixture.md) exercises authority and observer redaction with simulated renderer callbacks. It is a foundation for the graphical vertical slice; desktop rendering remains unimplemented.

```sh
cargo run -p rui-host-broker --example controlled
```
