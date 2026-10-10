import assert from "node:assert/strict";
import { readFile, readdir, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const read = (name) => readFile(resolve(root, name));
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const metadata = JSON.parse(await read("package.json"));
const runtime = await import(resolve(root, "dist/index.js"));
// Import succeeds in Node without document, location, WebSocket or WASM init.
assert.equal(typeof globalThis.document, "undefined");
const declarations = {};
for (const name of (await readdir(resolve(root, "dist"))).sort()) {
  if (name.endsWith(".d.ts"))
    declarations[name] = sha(await read("dist/" + name));
}
const wasm = await WebAssembly.compile(
  await read("wasm/rui_client_wasm_bg.wasm"),
);
const imports = WebAssembly.Module.imports(wasm);
assert(imports.every((item) => item.module !== "wasi_snapshot_preview1"));
const baseline = {
  exports: Object.keys(runtime).sort(),
  declarations,
  generatedDeclaration: sha(await read("wasm/rui_client_wasm.d.ts")),
  wasmImports: imports,
  wasmExports: WebAssembly.Module.exports(wasm),
};
if (process.argv.includes("--update")) {
  await writeFile(
    resolve(root, "scripts/api-baseline.json"),
    JSON.stringify(baseline, null, 2) + "\n",
  );
} else {
  assert.deepEqual(
    baseline,
    JSON.parse(await read("scripts/api-baseline.json")),
    "API/ABI changed: review version/changelog, then run compatibility:update",
  );
}
const crates = await readFile(resolve(root, "../Cargo.toml"), "utf8");
const crateVersion = /\[workspace.package\][\s\S]*?version = "([^"]+)"/.exec(
  crates,
)?.[1];
assert.equal(crateVersion, metadata.version);
const wasmManifest = await readFile(
  resolve(root, "../client-wasm/Cargo.toml"),
  "utf8",
);
const binding = /wasm-bindgen = "=([^"]+)"/.exec(wasmManifest)?.[1];
assert(binding);
const assets = {};
for (const name of [
  "rui_client_wasm.js",
  "rui_client_wasm.d.ts",
  "rui_client_wasm_bg.wasm",
  "rui_client_wasm_bg.wasm.d.ts",
])
  assets["wasm/" + name] = sha(await read("wasm/" + name));
await writeFile(
  resolve(root, "compatibility.json"),
  JSON.stringify(
    {
      package: metadata.name,
      version: metadata.version,
      crateVersion,
      wire: "RUI/0.1",
      wasmBindgen: binding,
      sessionSchemaSha256: sha(
        await readFile(resolve(root, "../spec/session.proto")),
      ),
      commandSchemaSha256: sha(
        await readFile(resolve(root, "../spec/rui.proto")),
      ),
      declarations,
      assets,
      noticesSha256: sha(await read("THIRD_PARTY_NOTICES.txt")),
    },
    null,
    2,
  ) + "\n",
);
console.log(
  JSON.stringify({
    package: metadata.name,
    version: metadata.version,
    exports: baseline.exports.length,
    importWithoutDom: true,
    apiAbiChecked: true,
  }),
);
