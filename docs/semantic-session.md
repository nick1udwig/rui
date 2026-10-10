# Semantic-only session boundary

The portable Rust actor, WASM binding and public `BrowserClient` accept explicit
`mirror` and `semanticOnly` modes. Mirror is the compatibility default. A semantic
consumer supplies no canvas or visual renderer:

```ts
const client = await BrowserClient.create({
  mode: 'semanticOnly', url, clientId: approvedClientId,
  credential: callerCredentialProvider, selection: 'fixture',
});
client.subscribe(event => {
  if (event.type === 'applied' || event.type === 'resumed') {
    const state = client.state;
    if (state) {
      consumeValidatedSemanticModel(state);
      client.semanticReady(state.revision);
    }
  }
});
await client.connect();
client.requestControl(); // Explicit; reconnect never restores authority.
```

Readiness means the consumer accepted this exact committed semantic revision; it
is not a presentation acknowledgement. Every committed update, missing-resource
transition, reset, release and disconnect clears readiness. Acquiring or issuing a
semantic command requires readiness for the current revision and the ordinary
host lease/permission checks. A hidden page releases authority; becoming visible
requires explicit consumption/readiness again. A consumer can use the applied event
to schedule its own DOM work and acknowledge only after that work succeeds.

Negotiation retains `mirror2d.core.v1`, `semantics.core.v1` and
`content.no-raster.v1`, adding the host's existing `semantics.only.v1` override.
It requires no glyph profile. The text-clock profile is offered as an optional
supported source contract. OpenView requests `semantic-only`. The host projects out
Scene, Binding and cursor content and all resources; it retains structurally
validated Surface topology and geometry. Semantic command construction uses target
identity, advertised schema/actions, current source model/text/focus clocks and lease
context, without a visual Binding or complete scene surface. Geometry remains source
metadata and does not select semantic controls.

`Presented` and hit testing reject in semantic mode. Raw input, composition and
resize families have no portable/browser API and are not advertised by this mode.
They cannot be unlocked by sending SemanticReady. Shared/native host authority tests
remain the family gate. `capabilities` exposes owned negotiated profiles, mode,
control permission and allowed semantic action names; an action still requires the
node's supported advertised schema and current lease action set.

Rust readiness/authority/reset tests and actual WASM transcript checks run in
Chromium 153, Firefox 155 and WebKit 26.6. The live session check runs on the pinned
GTK 4.22.4/Cairo 1.18.4 owned Xvfb display with Chromium 151. It discovers mapped
semantic ownership, role and schema-one actions, then activates a button, explicitly
focuses/edits Unicode text and activates a stable collection item. The same mapping
runs on the corpus and independent document editor. Labels appear only in assertions
of intended source effects. A scoped owned certificate SPKI trust exception serves
this test; it does not replace the separate production HTTPS/proxy verification.

The bounded exact authorized packet reports in
`fixtures/gtk/semantic-session-golden` contain no credentials: ClientHello bodies are
redacted. Python's independent full-record decoder replays each view scope and checks
the exported final state field-for-field. Reconnect crosses to a new view and receives
ResetRequired, then consumes a new snapshot without restoring control. Historical
semantic/mirror fixtures and the independent Python semantic client remain intact.

```sh
python3 demos/semantic/session_check.py --xvfb /path/to/Xvfb --output /tmp/new-corpus
python3 demos/semantic/session_check.py --xvfb /path/to/Xvfb --application editor --output /tmp/new-editor
```

This establishes S1's session mode for two cooperating applications. A generic DOM
presentation and its guarded controls/responsive/accessibility gates are subsequent
S2–S5 work. Full native actor consolidation and cache-pressure APIs remain tracked in
the client behavior matrix; this mode does not claim those migrations complete.
