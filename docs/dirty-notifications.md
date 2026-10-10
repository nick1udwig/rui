# Cooperating source invalidation (P2)

GTK source changes wake the Rust owner through a private Unix datagram socket,
separate from the ordered request/reply stream. A notification is exactly two
big-endian u64 values: source incarnation and monotonic dirty generation.
The source coalesces a burst to one wakeup and retains its current generation.
The receiver ignores old incarnations and already consumed generations, limits
each drain to 64 datagrams, and rejects malformed lengths. Each source restart
creates a new channel and identity.

The source's `consumed` reply boundary clears the pending edge only after the
corresponding coherent capture has been consumed. A newer generation schedules
another capture. A full socket leaves the level dirty and its queued wakeup
still causes reconciliation against current source state. Socket buffers are
bounded and charged by a separate 64 KiB source reservation. The owned GTK
process inherits only its endpoint; peers never receive this descriptor.

`GtkProducerPort` drains notifications on its single source owner alongside
command work, with the existing 5 ms lifecycle service interval. It permits one
synchronous capture at a time and does not move GTK/private-pipe work onto
network I/O. Captures still use the same submitted-node frame barrier. Ordered
command replies, source evidence, artifact consumption and native authority
fencing keep their previous boundaries. Notifications remaining after a remote
command's capture are retired by that capture's acknowledged generation.

The manual demo no longer requests source capture every 250 ms. Its 250 ms tick
only supervises startup and exit; source liveness and authority cleanup continue
independently on the owner. A direct `GtkCommandHost` integration can call
`refresh_notified()` from its owned event loop instead of using the port.

The declared cooperating application invalidates semantic callbacks, focus,
selection, size, mapping, sensitivity and CSS-class changes. Business callbacks
and custom visual drawing must call `CoherentSource.changed()`. Native object
registration supplies the supported property observation set. Arbitrary toolkit
animation, global style changes and unregistered custom paint remain outside
this scope; the demo disables animation and caret blinking. There is no claimed
detection bound for those excluded changes. Broader adapters must add observed
invalidation or a measured bounded reconciliation path before claiming support.

Regression checks exercise burst coalescing, a change during capture, channel
overflow, stale incarnation/generation, malformed datagrams, memory release,
source restart and guarded native editing. The actual native demo checks idle
publication stability, rapid edits and cleanup. Fresh whole-view and native
context checks still cross the common command ledger and actual GTK callbacks.
