export function resourceReply(source, request) {
  const batch = request.keys !== undefined;
  const keys = batch ? request.keys : [request.key];
  if (!Array.isArray(keys) || !keys.length || keys.length > 32 || new Set(keys).size !== keys.length || keys.some(key => typeof key !== 'string' || !/^[a-f0-9]{64}$/.test(key)) || (batch && request.key !== undefined)) throw Error('Resource keys rejected');
  const bodies = {};
  let bytes = 0;
  for (const key of keys) {
    const body = source.resource(key);
    if (typeof body !== 'string') throw Error('Resource unavailable');
    bytes += Buffer.byteLength(body);
    if (bytes > 8 * 1024 * 1024) throw Error('Resource response limit');
    bodies[key] = body;
  }
  return batch ? { bodies } : { body: bodies[keys[0]] };
}
