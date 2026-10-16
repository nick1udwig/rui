# RUI documentation

Start with [getting started](getting-started.md) to choose a workspace or demo, and [status and next steps](status.md) for current scope and priorities.
Commands in these guides run from the repository root unless stated otherwise.

## Using RUI

- [Existing desktop applications](../apps/desktop/README.md): share running Linux apps, pair a phone, choose windows, and inspect app compatibility.
- [Browser workspace](../apps/browser/README.md): control host Chromium, configure HTTPS/Tailscale, and use host files and login sessions.
- [Getting started](getting-started.md): prerequisites, semantic editor walkthrough, vector demos, and native clients on separate machines.
- [Semantic presentation](../demos/semantic/README.md): responsive controls, draft conflicts, accessibility, and the semantic-only contract.
- [Browser vector demo](../demos/web/README.md): source-layout mirroring, certificates, proxies, and static serving.
- [Development and verification](development.md): workspace checks, live GTK/browser tests, and small component examples.

## Components and integration

- [Protocol core](../spec/core-profile.md) and [wire codec](../wire/README.md): bounded records, atomic replication, validation, and authority.
- [Client core](../client-core/README.md), [WASM bindings](../client-wasm/README.md), and [browser library](../client-browser/README.md): the shared replica and platform APIs.
- [Browser integration](browser-integration.md), [capabilities](browser-capabilities.md), and [distribution](browser-distribution.md): public contracts, supported commands, packaging, and compatibility.
- [Host broker](../host-broker/README.md) and [transport](../transport/README.md): authorized projections, pairing, TLS/WebSocket, and resource recovery.
- [Producer SDK](../adapters/sdk/README.md) and [GTK adapter](../adapters/gtk4/README.md): cooperating sources, stable identities, and guarded native actions.
- [Read-only AT-SPI adapter](../adapters/atspi/README.md): the bounded observation baseline; interactive existing-app sharing is covered by the desktop workspace guide.
- [Client behavior matrix](client-behavior-matrix.md), [semantic sessions](semantic-session.md), [desktop controls](desktop-controls.md), and [desktop recovery](desktop-recovery.md): shared behavior and platform boundaries.

## Protocol design and profiles

The [original RFC](RFC.md) explains the design and its inspirations.
The profiles below document implemented contracts and extensions.

- [Core](../spec/core-profile.md), [rendering semantics](../spec/rendering-profile.md), and [native renderer](../spec/renderer-profile.md).
- [Resource format](../spec/resource-format.md), [glyph outlines](../spec/glyph-outline-profile.md), and [cursor](../spec/cursor-profile.md).
- [Input](../spec/input-profile.md), [isolated session shell](../spec/session-shell-profile.md), and [host fixture](../spec/host-fixture.md).
- Browser/desktop workspace extensions and their observed consistency are described in the respective app guides.

## Coverage, performance, and acceptance

- [Current status](status.md): distinguish existing-app semantics, managed browsing, and the original controlled prototype.
- [Application matrix](application-matrix.md), [GTK coverage](gtk-coverage.md), and [square-border lowering](gtk-square-borders.md): supported cooperating source content and explicit gaps.
- [Performance](performance.md), [large workloads](large-performance.md), and [performance gates](performance-gates.md): measurements, procedures, and unmet targets.
- [Dirty notifications](dirty-notifications.md), [incremental GTK publications](incremental-gtk.md), [browser rendering](browser-render-performance.md), and [browser lifecycle](browser-lifecycle-performance.md): changes and retained evidence.
- [Browser CI](browser-ci.md): reproducible three-engine checks and the pinned native runner.
- [Conformance guide](../conformance/README.md), [acceptance](../conformance/STATUS.md), and [case audit](../conformance/CASES.md): the original 79 cases and their evidence.
- [Additional session feasibility](additional-session-feasibility.md): the reviewed Hyprland hooks and why they do not establish vector desktop support.

## Implementation history

[IMPLEMENTATION.md](IMPLEMENTATION.md) records the prototype and subsequent client/producer work.
The [implementation plans and completion records](implementation-plans.md#implementation-record-2026-10-10) preserve the P/C/L/S workstreams, their original baselines, and validation commands.
[HANDOFF.md](HANDOFF.md) is a historical snapshot from 2026-10-02; its open gates and working-mode instructions describe that session.
The RFC and [conformance.yaml](../conformance.yaml) are original design inputs, while the acceptance records describe what was implemented within that scope.
