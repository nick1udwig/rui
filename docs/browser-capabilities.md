# Tested browser capability subset

The browser requests a small semantic-command subset. A node must advertise schema
one and the current host lease must permit the action. Unknown schemas, unavailable
text, mismatched value kinds, stale drafts/model versions and absent authority fail
before dispatch. A shared Rust helper does not itself enable a browser capability.

| Operation | Browser API and source evidence | Guards / limits |
| --- | --- | --- |
| Activate / explicit source focus | `activate`, `focus`; owned corpus and editor native callbacks | current revision/readiness, target version, lease/generation, allowed schema/action |
| Boolean SetValue | `setChecked`; registered editor ToggleButton, native GTK set_active with result verification | declared Boolean value and SetValue schema one, target/version/authority; numeric/text schemas are not browser APIs |
| Stable collection item | `activateItem(collection, item, modelVersion)` | current model version and membership, unique published collection owner; command includes target and model clocks |
| Explicit full text draft | `captureEdit`, `commitEdit` | original opaque epoch/view/lease/target/text-record/text/selection/focus stamp; scalar full replacement, 16 KiB commit ceiling, no NUL; no fresh retry |
| Scalar selection | `setSelection` | actual Available selectable model, source focus, text/target clocks, scalar bounds |
| Semantic mirror pointer | optional `bindPointer` | Rust inversion of the published viewport/surface/Binding; generic semantic action, never a raw input batch |
| Presentation | mirror CanvasRenderer or explicit semanticReady | actual mirror submission/opportunity versus consumer acceptance of current semantic model; modes never alias acknowledgements |

The caller can paste/delete/compose a locally committed Unicode string in a draft.
That is not source preedit or a per-keystroke text queue. Applied results remain
blocked until their causal revision has been consumed/presented. Rejected results
clear the wait; Unknown/Dispatched ambiguity requires explicit recovery. Pending
exact commands survive disconnect; retry uses original bytes. Hidden pages and
release/revocation disable mutation immediately. Source focus is an explicit action,
not a consequence of local DOM navigation.

Cursor rendering, raw pointer/wheel/touch, physical keyboard, source composition,
source/session resize/window operations and fingerprint-font mode remain disabled.
There are no browser dispatch APIs or advertised profiles for those families. Native
clients retain their separately tested families. Local viewport and semantic layout
changes do not invoke a source resize or scroll command. Outline glyphs remain the
mirror font baseline. Numeric/text SetValue requires an explicit source parameter
schema (including bounds), adapter evidence and browser API before enablement.

The reusable GTK adapter's `Controls.checkbox` registers a trusted ToggleButton or
CheckButton and exports its current Boolean/state plus SetValue/Focus. The private
native dispatcher accepts exactly one Boolean `checked` argument only for those
registered classes; the host authority declares Boolean schema only for Checkbox
nodes with an actual Boolean value. It uses the existing common ledger, source
incarnation, exact publication revision, current native target clock and lifecycle
fence. This adds no new wire vocabulary and no label-based native selection.
The owned editor uses ToggleButton, whose application-owned active state confirms
the native effect. CheckButton visual lowering is not separately claimed.

Three-engine actual-WASM tests cover typed command bytes and invalid schema/value
preflight, alongside all existing model/lifecycle/pixel tests. Live corpus/editor
checks reject stale collection clocks and invalid value targets. The editor performs
one Boolean effect and rejects another after release. Its bounded authorized packet
report in `fixtures/gtk/semantic-values.json` is independently replayed by Python;
credentials are redacted. Existing native editor stale text/focus/modal/removal
checks and SDK authority/publication tests remain enabled.
