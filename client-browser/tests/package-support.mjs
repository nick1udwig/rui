import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { cp, mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { createHash } from "node:crypto";
export const packageRoot = resolve(import.meta.dirname, "..");
export const repository = resolve(packageRoot, "..");
export const sha = (b) => createHash("sha256").update(b).digest("hex");

export async function prepareConsumer() {
  const temporary = await mkdtemp(resolve(tmpdir(), "rui-installed-consumer-"));
  try {
    execFileSync("npm", ["run", "build"], { cwd: packageRoot, stdio: "pipe" });
    const [pack] = JSON.parse(
      execFileSync(
        "npm",
        ["pack", "--ignore-scripts", "--json", "--pack-destination", temporary],
        {
          cwd: packageRoot,
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
        },
      ),
    );
    const runtimeNames = (await readdir(resolve(packageRoot, "dist"))).filter(
      (n) => /\.(js|d\.ts)$/.test(n),
    );
    const expected = [
      "package.json",
      "README.md",
      "CHANGELOG.md",
      "THIRD_PARTY_NOTICES.txt",
      "compatibility.json",
      ...runtimeNames.map((n) => "dist/" + n),
      ...[
        "rui_client_wasm.js",
        "rui_client_wasm.d.ts",
        "rui_client_wasm_bg.wasm",
        "rui_client_wasm_bg.wasm.d.ts",
      ].map((n) => "wasm/" + n),
    ];
    assert.deepEqual(
      pack.files.map((f) => f.path).sort(),
      expected.sort(),
      "runtime tarball includes unexpected/missing files",
    );
    const tarball = resolve(temporary, pack.filename);
    const consumer = resolve(temporary, "consumer");
    await cp(resolve(import.meta.dirname, "consumer"), consumer, {
      recursive: true,
    });
    execFileSync(
      "npm",
      [
        "install",
        tarball,
        "--ignore-scripts",
        "--no-audit",
        "--no-fund",
        ...(process.env.RUI_PACKAGE_OFFLINE === "1" ? ["--offline"] : []),
      ],
      { cwd: consumer, stdio: "pipe", timeout: 120000 },
    );
    // Resolve the installed package from the consumer, without repository aliases.
    const exports = JSON.parse(
      execFileSync(
        "node",
        [
          "--input-type=module",
          "-e",
          'console.log(JSON.stringify(Object.keys(await import("@rui/client-browser")).sort()))',
        ],
        { cwd: consumer, encoding: "utf8" },
      ),
    );
    const baseline = JSON.parse(
      await readFile(resolve(packageRoot, "scripts/api-baseline.json")),
    );
    assert.deepEqual(exports, baseline.exports);
    execFileSync("npm", ["run", "build"], {
      cwd: consumer,
      stdio: "pipe",
      timeout: 60000,
    });
    const installed = resolve(consumer, "node_modules/@rui/client-browser");
    const metadata = JSON.parse(
      await readFile(resolve(installed, "package.json")),
    );
    assert.equal(metadata.private, true);
    assert.equal(metadata.license, "UNLICENSED");
    const compatibility = JSON.parse(
      await readFile(resolve(installed, "compatibility.json")),
    );
    for (const [name, hash] of Object.entries(compatibility.assets))
      assert.equal(sha(await readFile(resolve(installed, name))), hash);
    const assets = (await readdir(resolve(consumer, "dist/assets"))).filter(
      (n) => n.endsWith(".wasm"),
    );
    assert.equal(assets.length, 1);
    return {
      temporary,
      consumer,
      wasm: assets[0],
      report: {
        tarballSha256: sha(await readFile(tarball)),
        tarballBytes: pack.size,
        unpackedBytes: pack.unpackedSize,
        files: expected,
        exports,
        version: metadata.version,
        installedOutsideRepository: true,
        nodeImportWithoutDom: true,
        declarationsCompile: true,
        productionBase: "/apps/rui/",
        compatibility,
      },
    };
  } catch (error) {
    if (error.stdout) console.error(error.stdout.toString());
    if (error.stderr) console.error(error.stderr.toString());
    await rm(temporary, { recursive: true, force: true });
    throw error;
  }
}
