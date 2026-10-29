import assert from "node:assert/strict";
import test from "node:test";
import { NativeWorker } from "../native-worker.mjs";
const program = `
let buffer = '';
process.stdin.on('data', data => {
  buffer += data;
  while (buffer.includes('\\n')) {
    const end = buffer.indexOf('\\n');
    const request = JSON.parse(buffer.slice(0, end));
    buffer = buffer.slice(end + 1);
    if (request.exit) process.exit(1);
    if (request.error) process.stdout.write(JSON.stringify({error:'rejected'})+'\\n');
    else process.stdout.write(JSON.stringify({pid:process.pid, value:request.value})+'\\n');
    if (request.dirty) process.stdout.write('{"dirty":true}\\n');
  }
});`;
test("native worker reuses one process, orders requests and restarts after failure", async () => {
  let changes = 0;
  const worker = new NativeWorker(
    () => changes++,
    [process.execPath, "-e", program],
  );
  try {
    const results = await Promise.all([
      worker.request({ value: 1, dirty: true }),
      worker.request({ value: 2 }),
    ]);
    assert.equal(results[0].pid, results[1].pid);
    assert.deepEqual(
      results.map((r) => r.value),
      [1, 2],
    );
    assert.equal(changes, 1);
    await assert.rejects(worker.request({ error: true }), /rejected/);
    assert.equal((await worker.request({ value: 3 })).pid, results[0].pid);
    await assert.rejects(worker.request({ exit: true }), /unavailable/);
    assert.notEqual((await worker.request({ value: 4 })).pid, results[0].pid);
  } finally {
    worker.close();
  }
  await assert.rejects(worker.request({}), /unavailable/);
});
