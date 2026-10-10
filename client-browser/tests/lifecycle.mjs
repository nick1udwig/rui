import assert from "node:assert/strict";

export async function measureLifecycle({
  page,
  browser,
  address,
  credential,
  wait,
  ready,
}) {
  const measure = (operation) =>
    page.evaluate(
      ({ operation, address, credential }) =>
        window.consumer.measure(operation, address, credential),
      { operation, address, credential },
    );
  const inspect = () => page.evaluate(() => window.consumer.inspect());
  const diagnostics = () => page.evaluate(() => window.consumer.diagnostics());
  const rows = {
    cold: [await measure("cold")],
    reconnect: [],
    cacheLoss: [],
    lifecycle: [],
  };
  for (let i = 0; i < 3; i++) {
    rows.reconnect.push(await measure("reconnect"));
    assert.equal((await inspect()).hasControl, false);
  }
  // Compare the complete owned committed records outside the timing interval.
  const before = await page.evaluate(() =>
    JSON.stringify(window.consumer.inspect().state, (_key, value) =>
      value instanceof Map
        ? Object.fromEntries(value)
        : typeof value === "bigint"
          ? value.toString()
          : value,
    ),
  );
  for (let i = 0; i < 3; i++) {
    rows.cacheLoss.push(await measure("cacheLoss"));
    const after = await page.evaluate(() =>
      JSON.stringify(window.consumer.inspect().state, (_key, value) =>
        value instanceof Map
          ? Object.fromEntries(value)
          : typeof value === "bigint"
            ? value.toString()
            : value,
      ),
    );
    assert.equal(after, before, "cache loss changed committed records");
  }
  await page.evaluate(() => window.consumer.requestControl());
  await wait(inspect, (s) => ready(s) && s.hasControl);
  const prior = (await inspect()).revision;
  await page.evaluate(() => {
    window.consumer.holdResults();
    window.consumer.activate();
  });
  await wait(
    async () => ({ state: await inspect(), diagnostics: await diagnostics() }),
    ({ state, diagnostics }) =>
      state.revision > prior &&
      state.presented === state.revision &&
      state.pending === 1 &&
      diagnostics.heldResultBytes > 0,
  );
  rows.pendingOutcome = {
    before:
      "one exact command; Applied delivery held after authoritative publication",
    recovery: await measure("reconnect"),
    after: "queried Applied; no fresh retry or authority",
  };
  assert.equal((await inspect()).pending, 0);
  assert.equal((await inspect()).hasControl, false);
  const stages = await diagnostics();
  const cdp = await page.context().newCDPSession(page);
  const heaps = [];
  for (let i = 0; i < 25; i++) {
    const ownership = await diagnostics();
    await page.evaluate(() => window.consumer.dispose());
    const disposed = await diagnostics();
    assert.equal(disposed.memoryUsed, 0);
    assert.equal(disposed.renderer.ownedBytes, 0);
    rows.lifecycle.push({
      cycle: i + 1,
      before: {
        rustPeak: ownership.memoryPeak,
        rendererPeak: ownership.renderer.peakBytes,
      },
      disposedRust: disposed.memoryUsed,
      disposedRenderer: disposed.renderer.ownedBytes,
      reconnect: await measure("cold"),
    });
    if (i % 5 === 4) {
      await cdp.send("HeapProfiler.collectGarbage");
      heaps.push({
        cycle: i + 1,
        ...(await cdp.send("Runtime.getHeapUsage")),
        dom: await cdp.send("Memory.getDOMCounters"),
      });
    }
  }
  // This is a bounded stabilization check, distinct from Rust/renderer ownership.
  assert(
    heaps.at(-1).usedSize <= heaps[0].usedSize + 8 * 1024 * 1024,
    "post-GC JS heap did not stabilize",
  );
  assert(
    heaps.at(-1).dom.nodes <= heaps[0].dom.nodes + 64,
    "DOM owners accumulated",
  );
  assert(
    heaps.at(-1).backingStorageSize <=
      heaps[0].backingStorageSize + 8 * 1024 * 1024,
    "post-GC backing storage did not stabilize",
  );
  assert.equal(
    heaps.at(-1).dom.jsEventListeners,
    heaps[0].dom.jsEventListeners,
  );
  let hidden;
  if (process.env.RUI_BROWSER_HEADED === "1") {
    await page.evaluate(() => window.consumer.requestControl());
    await wait(inspect, (s) => ready(s) && s.hasControl);
    const coverPromise = page.context().waitForEvent("page");
    await page.evaluate(() => window.open("about:blank", "_blank"));
    const cover = await coverPromise;
    await cover.bringToFront();
    await page.waitForFunction(() => document.visibilityState === "hidden");
    await wait(inspect, (s) => !s.hasControl);
    const started = performance.now();
    const refill = measure("cacheLoss");
    await wait(inspect, (s) => s.resourcesReady && s.presented === 0n);
    await new Promise((done) => setTimeout(done, 250));
    assert.equal(
      (await inspect()).presented,
      0n,
      "hidden client claimed presentation",
    );
    await page.bringToFront();
    const visibleRecovery = await refill;
    assert.equal((await inspect()).hasControl, false);
    hidden = {
      controlReleased: true,
      verifiedRefillWithoutPresentation: true,
      visibleRecovery,
      wallMsIncludingHiddenInterval: performance.now() - started,
      authorityRestored: false,
    };
    await cover.close();
  }
  await cdp.detach();
  const final = await diagnostics();
  await page.evaluate(() => window.consumer.dispose());
  const disposed = await diagnostics();
  assert.equal(disposed.memoryUsed, 0);
  assert.equal(disposed.renderer.ownedBytes, 0);
  return {
    rows,
    heaps,
    hidden,
    stages,
    final,
    disposal: {
      rust: disposed.memoryUsed,
      renderer: disposed.renderer.ownedBytes,
    },
    conditions:
      "Presentation durations use the browser monotonic clock and actual visible frame event. Cold starts include WASM initialization but exclude HTML/JS download. State comparison/heap GC are outside those intervals. Only authenticated producer-busy responses retry for at most 2s, with the 50ms retry wait included. GTK reconnect uses a fresh-scope snapshot, not retained GTK delta resume. Forced GC/8MiB JS/backing-storage drift and 64-node DOM stabilization are separate from negotiated ownership ceilings.",
  };
}
