import { rm } from "node:fs/promises";
// Only the generated package output. Retired source modules must not survive in
// a later tarball just because TypeScript does not remove old output files.
await rm(new URL("../dist/", import.meta.url), { recursive: true, force: true });
