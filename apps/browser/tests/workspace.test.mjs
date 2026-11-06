import test from "node:test";
import assert from "node:assert/strict";
import {
  mkdtemp,
  writeFile,
  mkdir,
  symlink,
  rm,
  readFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:http";
import { Workspace, safeFile, navigationURL } from "../workspace.mjs";

test("address navigation defaults bare hosts to HTTPS and preserves explicit URLs", () => {
  for (const [address, expected] of [
    ["youtube.com", "https://youtube.com/"],
    ["  youtube.com/results?search_query=browser#top  ", "https://youtube.com/results?search_query=browser#top"],
    ["//youtube.com", "https://youtube.com/"],
    ["youtube.com:8443/watch", "https://youtube.com:8443/watch"],
    ["localhost:8443", "https://localhost:8443/"],
    ["[::1]:8443", "https://[::1]:8443/"],
    ["https://youtube.com", "https://youtube.com/"],
    ["http://localhost:8080/path", "http://localhost:8080/path"],
    ["about:blank", "about:blank"],
  ]) assert.equal(navigationURL(address), expected);
  for (const address of ["", "   ", null, "not a host", "javascript:alert(1)", "data:text/html,hello", "file:///tmp/private", "about:config"])
    assert.throws(() => navigationURL(address));
});

test("host file selection rejects traversal and symlinks", async () => {
  const root = await mkdtemp(join(tmpdir(), "rui-files-"));
  try {
    await mkdir(join(root, "shared"));
    await writeFile(join(root, "private"), "private");
    await symlink(join(root, "private"), join(root, "shared", "link"));
    await assert.rejects(safeFile(join(root, "shared"), join(root, "private")));
    await assert.rejects(
      safeFile(join(root, "shared"), join(root, "shared", "link")),
    );
  } finally {
    await rm(root, { recursive: true });
  }
});
test(
  "real Chromium observes and guards page actions, tabs, secrets and files",
  { timeout: 90000 },
  async () => {
    const directory = await mkdtemp(join(tmpdir(), "rui-browser-test-"));
    const server = createServer((req, res) => {
      if (req.url === "/download") {
        res.writeHead(200, {
          "Content-Type": "application/octet-stream",
          "Content-Disposition": "attachment; filename=report.txt",
        });
        res.end("download contents");
        return;
      }
      if (req.url === "/frame") {
        res.end("<!doctype html><label>Frame field<input id=framed></label>");
        return;
      }
      res.setHeader("Content-Type", "text/html");
      res.end(
        `<!doctype html><title>Workspace test</title><main><h1>Fixture page</h1><p>Readable paragraph</p><ul><li>First item</li><li>Second item</li></ul></main><label>Name<input id=name value=before></label><label>Password<input id=password type=password></label><button onclick="document.getElementById('result').textContent='Clicked'">Increment</button><p id=result>Ready</p><label>Reviewed<input id=checked type=checkbox></label><label>Color<select id=color><option>Red</option><option>Blue</option></select></label><input id=file type=file aria-label="Upload document"><a href="/next">Next page</a><a href="/download" download>Download report</a><div contenteditable="plaintext-only" aria-label="Notes">Initial note</div><label>Quantity<input id=quantity type=number min=1 max=5 value=2></label><label>Day<input id=day type=date value=2026-10-15></label><iframe src="/frame" title="Embedded form"></iframe><button onclick="alert('Hello phone')">Open dialog</button>`,
      );
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    process.env.RUI_FILES_DIR = directory;
    process.env.RUI_HEADLESS = "1";
    const workspace = await Workspace.launch(directory);
    try {
      const page = workspace.active;
      await page.goto(`http://127.0.0.1:${server.address().port}/`);
      const dispatch = async (label, value = null) => {
        const c = await workspace.capture();
        const n = c.nodes.find((n) => n.label === label && n.actions.length);
        assert.ok(
          n,
          `missing ${label}: ${c.nodes.map((n) => n.label).join(", ")}`,
        );
        const result = await workspace.dispatch({
          target: n.id,
          version: n.version,
          action: n.actions[0],
          value,
          deadlineMs: 2000,
        });
        assert.equal(result.status, "dispatched", label);
        return n;
      };
      const structured = await workspace.capture();
      assert.equal(
        structured.nodes.filter((n) => n.label === "Name").length,
        1,
      );
      assert.equal(
        new Set(structured.nodes.map((n) => n.id)).size,
        structured.nodes.length,
      );
      assert.ok(
        structured.nodes.some(
          (n) => n.kind === "view:heading1" && n.label === "Fixture page",
        ),
      );
      for (const kind of [
        "view:document",
        "view:paragraph",
        "view:list",
        "view:item",
      ])
        assert.ok(
          structured.nodes.some((n) => n.kind === kind),
          kind,
        );
      await page.evaluate(() => {
        const button = document.createElement('button');
        button.id = 'document-target'; button.textContent = 'Document action';
        button.onclick = () => { button.textContent = 'Document action received'; };
        document.body.append(button);
      });
      const source = workspace.pages.get(page).document;
      const snapshot = await source.capture();
      const find = node => node.attrs?.id === 'document-target' ? node : (node.children || []).map(find).find(Boolean);
      const documentButton = find(snapshot.root);
      const documentTarget = (await workspace.capture()).nodes.find(n => n.kind === 'document-command');
      const command = { generation: snapshot.generation, revision: snapshot.revision, node: documentButton.id, action: 'click' };
      const sendDocument = value => workspace.dispatch({ target: documentTarget.id, version: documentTarget.version, action: 'SetValue', value, deadlineMs: 2000 });
      assert.equal((await sendDocument('{invalid')).status, 'rejected');
      assert.equal((await sendDocument(JSON.stringify({ ...command, generation: 'stale' }))).status, 'rejected');
      await page.locator('#document-target').evaluate(n => n.hidden = true);
      assert.equal((await sendDocument(JSON.stringify(command))).status, 'rejected', 'An unclickable target has no uncertain action');
      await page.locator('#document-target').evaluate(n => n.hidden = false);
      const capture = workspace.capture;
      workspace.capture = async () => { throw Error('Actions must not wait for a full accessibility crawl'); };
      try { assert.equal((await sendDocument(JSON.stringify(command))).status, 'dispatched'); }
      finally { workspace.capture = capture; }
      assert.equal(await page.locator('#document-target').textContent(), 'Document action received');
      await page.frames()[1].goto(`http://127.0.0.1:${server.address().port}/frame`);
      assert.equal((await sendDocument(JSON.stringify(command))).status, 'dispatched', 'Child navigation does not retire the parent document target');
      assert.equal((await workspace.capture()).nodes.find(n => n.kind === 'document-command').id, documentTarget.id);
      await dispatch("Increment");
      assert.equal(await page.locator("#result").textContent(), "Clicked");
      await dispatch("Name", "café Ω 𝄞");
      assert.equal(await page.locator("#name").inputValue(), "café Ω 𝄞");
      const before = (await workspace.capture()).nodes.find(
        (n) => n.label === "Name",
      );
      await page.locator("#name").fill("local change");
      assert.equal(
        (
          await workspace.dispatch({
            target: before.id,
            version: before.version,
            action: "SetValue",
            value: "stale",
            deadlineMs: 2000,
          })
        ).status,
        "rejected",
      );
      await dispatch("Reviewed", true);
      assert.equal(await page.locator("#checked").isChecked(), true);
      await dispatch("Blue");
      assert.equal(await page.locator("#color").inputValue(), "Blue");
      const password = (await workspace.capture()).nodes.find(
        (n) => n.kind === "password",
      );
      assert.ok(password, "password control");
      const scope = {
        client: "client",
        target: password.id,
        version: password.version,
        lease: "lease",
        generation: 1,
        epoch: "epoch",
      };
      const token = workspace.stage(scope, "private-test-value");
      assert.equal(
        (
          await workspace.dispatch({
            ...scope,
            action: "SetValue",
            value: token,
            deadlineMs: 2000,
          })
        ).status,
        "dispatched",
      );
      assert.equal(
        await page.locator("#password").inputValue(),
        "private-test-value",
      );
      assert.ok(
        !JSON.stringify(await workspace.capture()).includes(
          "private-test-value",
        ),
      );
      assert.equal(
        (
          await workspace.dispatch({
            ...scope,
            action: "SetValue",
            value: token,
            deadlineMs: 2000,
          })
        ).status,
        "rejected",
      );
      await writeFile(join(directory, "upload.txt"), "document");
      await dispatch("Upload document");
      await dispatch("upload.txt");
      assert.equal(
        await page.locator("#file").evaluate((e) => e.files[0].name),
        "upload.txt",
      );
      await dispatch("Frame field", "inside frame");
      assert.equal(
        await page.frameLocator("iframe").locator("#framed").inputValue(),
        "inside frame",
      );
      await dispatch("Notes", "Plain café");
      assert.equal(
        await page.locator("[contenteditable]").textContent(),
        "Plain café",
      );
      await dispatch("Day", "2026-10-16");
      assert.equal(await page.locator("#day").inputValue(), "2026-10-16");
      const quantity = (await workspace.capture()).nodes.find(
        (n) => n.label === "Quantity" && n.actions.length,
      );
      assert.equal(
        (
          await workspace.dispatch({
            target: quantity.id,
            version: quantity.version,
            action: "SetValue",
            value: "99",
            deadlineMs: 2000,
          })
        ).status,
        "rejected",
      );
      assert.equal(await page.locator("#quantity").inputValue(), "2");
      await dispatch("Quantity", "3");
      await dispatch("Open dialog");
      assert.ok(workspace.dialog);
      await dispatch("Accept dialog");
      assert.equal(workspace.dialog, undefined);
      await dispatch("Download report");
      for (
        let i = 0;
        i < 100 && !workspace.downloads[0]?.status.startsWith("Saved");
        i++
      )
        await new Promise((r) => setTimeout(r, 50));
      assert.equal(workspace.downloads.length, 1);
      assert.ok(workspace.downloads[0].status.startsWith("Saved on host: "));
      assert.equal(
        await readFile(
          workspace.downloads[0].status.slice("Saved on host: ".length),
          "utf8",
        ),
        "download contents",
      );
      const oldButton = (await workspace.capture()).nodes.find(
        (n) => n.label === "Increment",
      );
      await page.goto(`http://127.0.0.1:${server.address().port}/next`);
      assert.equal(
        (
          await workspace.dispatch({
            target: oldButton.id,
            version: oldButton.version,
            action: "Activate",
            deadlineMs: 2000,
          })
        ).status,
        "rejected",
      );
      const newPassword = (await workspace.capture()).nodes.find(
        (n) => n.kind === "password",
      );
      const newScope = {
        ...scope,
        target: newPassword.id,
        version: newPassword.version,
      };
      const expired = workspace.stage(newScope, "expired");
      workspace.secrets.get(expired).expires = 0;
      assert.equal(
        (
          await workspace.dispatch({
            ...newScope,
            value: expired,
            action: "SetValue",
            deadlineMs: 2000,
          })
        ).status,
        "rejected",
      );
      const fenced = workspace.stage(newScope, "wrong lease");
      assert.equal(
        (
          await workspace.dispatch({
            ...newScope,
            lease: "other",
            value: fenced,
            action: "SetValue",
            deadlineMs: 2000,
          })
        ).status,
        "rejected",
      );
      assert.equal(await page.locator("#password").inputValue(), "");
      await dispatch("New tab");
      assert.equal(workspace.pages.size, 2);
      const c = await workspace.capture();
      assert.ok(c.nodes.some((n) => n.label.includes("Workspace test")));
    } finally {
      await workspace.context.close();
      await new Promise((resolve) => server.close(resolve));
      await rm(directory, { recursive: true, force: true });
    }
  },
);

test("observations reuse clean captures and retain invalidations during capture", async () => {
  const context = { on() {}, pages: () => [] };
  const workspace = new Workspace(context, "", "");
  let captures = 0;
  workspace.capture = async () => ({ sequence: ++captures });
  const first = await workspace.observe();
  assert.equal(await workspace.observe(), first);
  workspace.invalidate();
  assert.notEqual(await workspace.observe(), first);
  workspace.observationTime = 0;
  assert.equal((await workspace.observe()).sequence, 3);
  workspace.invalidate();
  workspace.capture = async () => {
    workspace.invalidate();
    return { sequence: ++captures };
  };
  await workspace.observe();
  await workspace.observe();
  assert.equal(captures, 5);
});
