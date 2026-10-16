import { createConnection } from "node:net";

// Decode once: a stream chunk may end in the middle of a UTF-8 character.
export async function readJson(stream, { limit = 65536, line = false } = {}) {
  const chunks = [];
  let size = 0;
  for await (const chunk of stream) {
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    size += bytes.length;
    if (size > limit) throw Error("JSON response or request limit");
    chunks.push(bytes);
    if (line && bytes.includes(10)) {
      const data = Buffer.concat(chunks, size);
      return JSON.parse(data.subarray(0, data.indexOf(10)).toString("utf8"));
    }
  }
  if (line) throw Error("Host disconnected before completing its response");
  return JSON.parse(Buffer.concat(chunks, size).toString("utf8"));
}

export async function requestJson(
  path,
  request,
  { timeout = 6000, limit = 8192 } = {},
) {
  const socket = createConnection(path);
  socket.setTimeout(timeout, () => socket.destroy(Error("Host deadline")));
  socket.once("connect", () => socket.write(JSON.stringify(request) + "\n"));
  try {
    const result = await readJson(socket, { limit, line: true });
    if (result.error) throw Error(result.error);
    return result;
  } finally {
    socket.destroy();
  }
}
