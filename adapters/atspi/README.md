# Bounded observed accessibility adapter

`WorkerPort` runs blocking AT-SPI queries in a disposable process. A trusted
launcher supplies an explicit accessibility bus and the PIDs of at most two
owned applications; `ObservedProjection` additionally requires a fresh process
incarnation for each source replacement. The worker binds each accessible
application to its bus connection's Unix PID. Titles and geometry never choose
an application, compositor surface or input destination.

Call `request('crawl', pids=[...])` periodically, then poll without blocking the
publication loop. Only a completed `snapshot` candidate may enter
`ObservedProjection.publish(candidate, {pid: incarnation})`. There is one
in-flight crawl. The caller owns reconciliation cadence and source process
supervision; removing an exited process from the trusted scope retires its public
objects. An empty scope publishes only the stable Session. A failed or timed-out
crawl returns unavailable and leaves the last validated publication unchanged.
The caller must expose that availability status. This component does not grant
control or serve a network session.

The worker subscribes only to selected application object events and compares two
complete structural samples. It checks parents, child counts, duplicate/cyclic
ownership, and children after traversal; at most one bounded retry follows a
race. Periodic crawls recover missed events. Matching samples do not establish
an atomic source boundary: every logical surface remains `OBSERVED`, visual
coverage is `Unsupported`, and semantic coverage is `Partial`. Semantics contain
roles, bounded names and hierarchy only. Password-role names are redacted;
text/value/action interfaces are not read. Native/compositor surface association
is explicitly unresolved. No scene, input region, routing token, semantic action
or raw input mode is exported.

Public identities are independent of native object paths. Removal retires an ID;
a reused path or replacement source receives a new ID. A Session and its random
binding survive source replacement. Publication staging is atomic, including
failure from identity exhaustion or invalid structure. Separate observation
tokens describe the sampled hierarchy and sampled geometry without asserting
coherence between them.

## Bounds and ownership

- One private `SOCK_SEQPACKET` job; 4 KiB request, 128 KiB response and public
  envelope. No native handle enters the publisher.
- At most 16 registry applications inspected, two authorized sources, four
  frames per source, 128 nodes per frame, depth 16, and 256-byte strings. The
  public projection additionally limits total records to 265 including Session.
- At most 1,024 calls and two seconds per crawl; individual calls time out after
  200 ms. The owner kills a worker after three seconds without waiting in `poll`.
- Worker address space is limited to 256 MiB before GLib initialization. This
  also bounds opaque native allocations made before reply sizes can be checked.
  Python/native allocation limits are separate from Rust `MemoryBudget` counters.
- At most 4,095 public IDs after the reserved Session, eight source incarnations,
  and 63-bit publication/request/event counters; exhaustion fails closed.
- `close()` is explicit teardown and may wait up to one second to reap the killed
  worker. Fixture barrier/event-suppression operations are available only on the
  inherited private worker socket, never a public RUI endpoint.

## Actual native proof

```sh
python3 fixtures/atspi/native_check.py \
  --xvfb /path/to/Xvfb --output /tmp/rui-atspi-new-empty-output
python3 -m unittest adapters.atspi.test_observer adapters.atspi.test_trace
cargo test --offline -p rui-producer-sdk --test atspi_trace
```

The runner owns an authenticated Xvfb, private session bus, separate accessibility
bus and registry, two GTK applications and the worker. It never connects to the
user's accessibility bus or display. The six retained, hash-bound captures in
`fixtures/atspi/golden` cover two applications with identical overlapping titles,
eight deliberately dropped real events, an actual widget replacement during
traversal, a SIGSTOP-stalled application, recovery, process exit and replacement.
The owner continued polling during the stalled call; retained publications pass
both Rust graph validation and the independent handwritten Python reducer.

The retained run used GTK 4.22.4, AT-SPI 2.60.6, GLib 2.88.3 and Python 3.14.7.
It observed one reconciliation retry, 184 owner polls during the stopped source,
and a maximum `poll()` duration of 0.286 ms. These are fixture observations, not
real-time guarantees or native mirror presentation measurements. The proof does
not establish arbitrary application coverage, compositor association, network
integration or interactive control.
