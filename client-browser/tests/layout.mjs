import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
export async function checkLayout(page, engine, version) {
  const output = resolve(import.meta.dirname, "../test-output/semantic-layout");
  await mkdir(output, { recursive: true });
  const observations = [];
  for (const app of [0, 1]) {
    await page.evaluate(async (app) => {
      const { SemanticControls } = await import("/dist/index.js");
      const { Client } = await import("/tests/semantic-controls.mjs");
      const { model } = await import("/tests/semantic.mjs");
      window.layout?.controls.dispose();
      document.body.replaceChildren();
      const link = document.createElement("link");
      link.rel = "stylesheet";
      link.href = "/semantic-demo.css";
      document.head.append(link);
      const main = document.createElement("main"),
        root = document.createElement("div"),
        status = document.createElement("p");
      main.append(status, root);
      document.body.append(main);
      const fixtures = await (await fetch("/wasm/semantic-views.json")).json();
      const state = model(fixtures[app].state);
      for (const r of state.records.values())
        if ("Semantic" in r)
          r.Semantic.label = `${r.Semantic.role} — long name ${"אב العربية é 😀 ".repeat(5)}`;
      const surfaces = [...state.records.values()]
        .filter((r) => "Surface" in r)
        .map((r) => r.Surface);
      if (surfaces[1]) {
        surfaces[1].mapped = true;
        surfaces[1].modal_for = { kind: "Surface", id: surfaces[0].id };
      }
      for (const r of state.records.values())
        if ("Semantic" in r && r.Semantic.collection) {
          r.Semantic.collection.materialization = "Partial";
          r.Semantic.collection.total_count = 1000n;
        }
      const client = new Client(state),
        controls = new SemanticControls(client, root, status);
      client.emit({ type: "applied", revision: state.revision });
      window.layout = {
        client,
        controls,
        root,
        state,
        application: fixtures[app].application,
        baseline: JSON.stringify(state, (_k, v) =>
          v instanceof Map ? [...v] : typeof v === "bigint" ? String(v) : v,
        ),
      };
    }, app);
    await page.locator("textarea").first().waitFor();
    for (const width of [320, 800, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      const measured = await page.evaluate(() => {
        const { root, client, state, baseline } = window.layout;
        const dialog = root.querySelector("[role=dialog]");
        const snapshot = JSON.stringify(state, (_k, v) =>
          v instanceof Map ? [...v] : typeof v === "bigint" ? String(v) : v,
        );
        return {
          application: window.layout.application,
          viewport: innerWidth,
          width: document.documentElement.scrollWidth,
          nodes: root.querySelectorAll("[data-semantic-id]").length,
          unchanged: snapshot === baseline,
          commands: client.commands.length,
          dialog: !!dialog,
          localModal: dialog?.getAttribute("aria-modal"),
          partial: root.textContent.includes("of 1000"),
        };
      });
      assert(
        measured.width <= width + 1,
        "horizontal overflow " + JSON.stringify(measured),
      );
      assert(
        measured.unchanged && measured.commands === 0 && measured.partial,
        "local reflow changed source or completeness",
      );
      if (measured.dialog) assert.equal(measured.localModal, "false");
      const ax = await page.locator("main").ariaSnapshot();
      assert(
        ax.includes("textbox") && ax.includes("long name"),
        "accessible source name missing",
      );
      if (measured.dialog)
        assert(ax.includes("dialog"), "dialog omitted from accessibility tree");
      observations.push({ ...measured, accessibleNames: true });
      if (engine === "chromium" && (width === 320 || width === 1440))
        await page.screenshot({
          path: resolve(output, `${measured.application}-${width}.png`),
          fullPage: true,
        });
    }
    await page.setViewportSize({ width: 320, height: 900 });
    await page.evaluate(() => {
      document.documentElement.style.fontSize = "200%";
    });
    assert(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth + 1,
      ),
      "enlarged text overflow",
    );
    await page.evaluate(() => {
      document.documentElement.style.fontSize = "";
      window.layout.root.querySelector("textarea").focus();
    });
    await page.keyboard.press("Tab");
    assert(
      await page.evaluate(() =>
        window.layout.root.contains(document.activeElement),
      ),
      "keyboard traversal escaped semantic view",
    );
    assert(
      await page.evaluate(() => window.layout.client.commands.length === 0),
      "observer traversal dispatched",
    );
    await page.evaluate(() => {
      const c = window.layout.client;
      c.held = true;
      c.emit({ type: "control", granted: true });
      window.layout.root.querySelector("[data-action=Activate]").focus();
    });
    const name = await page
      .locator("[data-action=Activate]")
      .first()
      .getAttribute("aria-label");
    assert(name.includes("long name"), "action lacks source name");
    await page.keyboard.press("Enter");
    assert(
      await page.evaluate(() => window.layout.client.commands.length === 1),
      "native keyboard activation failed",
    );
    assert(
      await page
        .locator("[role=status]")
        .first()
        .textContent()
        .then((s) => s.includes("Pending")),
      "pending announcement missing",
    );
    if (engine === "chromium") {
      const session = await page.context().newCDPSession(page);
      const { nodes } = await session.send("Accessibility.getFullAXTree");
      assert(
        nodes.some(
          (n) =>
            ["textbox", "textField"].includes(n.role?.value) &&
            n.name?.value.includes("long name"),
        ),
        "browser accessibility API lost text label",
      );
      assert(
        nodes.some((n) => n.role?.value === "status"),
        "browser accessibility API lost status",
      );
      await session.detach();
    }
    await page.evaluate(() => {
      window.layout.controls.dispose();
      delete window.layout;
    });
  }
  const report = {
    engine,
    version,
    applications: 2,
    viewports: [320, 800, 1440],
    enlargedText: "200%",
    keyboard: true,
    localSourceFocusSeparate: true,
    accessibility:
      engine === "chromium"
        ? "Browser AX API and aria snapshot"
        : "aria snapshot",
    physicalScreenReader: false,
    mobileDevice: false,
    observations,
  };
  await writeFile(
    resolve(output, `${engine}.json`),
    JSON.stringify(report, null, 2) + "\n",
  );
  return {
    responsiveLayouts: true,
    keyboardControls: true,
    accessibleNames: true,
    localReflowOnly: true,
  };
}
