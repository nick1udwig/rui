# Retained desktop recovery

The desktop network worker preserves its committed receiver across socket loss.
It clears the lease, selected target, typing/composition context, pending rendered
frame and Presented marker, and advances the UI generation. Delayed old-generation
paint/input callbacks cannot restore readiness or modify the new editing state.
The toolbar provides explicit Reconnect and Clear cache operations.

An ambiguous send retains its exact command bytes and family in a charged FIFO:
at most 32 entries, with capacity reserved before encoding, sequence consumption
or sending. After authenticated reconnect and OpenView, the worker retries those
same bytes under their original lease and sequence, draining interleaved recovery
publications until each result arrives. This recovers an existing ledger result
or consumes a rejection for a command that never arrived. It never rewrites an
uncertain command under a new lease. A confirmed new epoch retires old pending
commands and starts a fresh sequence; narrower incompatible limits keep input
paused. Failed rebind allocation preserves the previous queue owner.

Recovery is cancellable and bounded. An unsuccessful attempt keeps retained
state and waits for explicit Reconnect. Connection changes rebind frame, typing
and command reservations to the new receiver budget. A verified restored frame
must receive a fresh renderer callback before automatic control acquisition.
Socket failures during presentation, acquire/release, renewal, publication
acknowledgments and source commands use the same recovery path.

`client-desktop/tests/recovery_worker.rs` verifies actual TLS loss after a source
effect but before its reply, link cuts during presentation/control, retained
resume, cache reacquisition and rejection of delayed UI callbacks. Queue tests
check reserve-before-encode, bounds, exact bytes and atomic budget rebind; library
tests check lifecycle generation handling. These worker tests explicitly simulate
presentation callbacks. Actual GTK after-paint recovery, broader native controls
and action-to-Presented measurements remain separate integration gates.
