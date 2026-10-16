// One consumer owns the stream, including requests buffered while a handler awaits.
export async function serveFrames(socket, handle) {
  let pending = Buffer.alloc(0);
  try {
    for await (const chunk of socket) {
      pending = Buffer.concat([pending, chunk]);
      if (pending.length > 1024 * 1024) throw Error("RPC queue limit");
      while (!socket.destroyed && pending.length >= 4) {
        const size = pending.readUInt32BE(0);
        if (size > 65536) throw Error("RPC request limit");
        if (pending.length < size + 4) break;
        const request = JSON.parse(pending.subarray(4, size + 4));
        pending = pending.subarray(size + 4);
        const result = await handle(request);
        if (socket.destroyed) return;
        const data = Buffer.from(JSON.stringify(result));
        if (data.length > 1024 * 1024) throw Error("RPC response limit");
        const header = Buffer.alloc(4);
        header.writeUInt32BE(data.length);
        // Waiting for the write also bounds queued responses for a slow reader.
        await new Promise((resolve, reject) => {
          socket.write(Buffer.concat([header, data]), (error) =>
            error ? reject(error) : resolve(),
          );
        });
      }
    }
    if (pending.length) throw Error("Incomplete RPC frame");
  } catch {
    socket.destroy();
  }
}
