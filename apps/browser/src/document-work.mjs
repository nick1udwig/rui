export function taskPool(limit) {
  let active = 0;
  const waiting = [];
  return async task => {
    if (active < limit) active++;
    else await new Promise(resolve => waiting.push(resolve));
    try { return await task(); }
    finally {
      const next = waiting.shift();
      if (next) next(); else active--;
    }
  };
}

export async function concurrent(items, limit, visit) {
  let next = 0, failed = false, failure;
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length && !failed) {
      const index = next++;
      try { await visit(items[index], index); } catch (error) { failed = true; failure = error; }
    }
  }));
  if (failed) throw failure;
}

export function resourceBatches(entries) {
  const batches = [];
  let batch = [], bytes = 0;
  for (const entry of entries) {
    if (batch.length && (batch.length === 32 || bytes + entry[1].bytes > 2 * 1024 * 1024)) {
      batches.push(batch); batch = []; bytes = 0;
    }
    batch.push(entry);
    bytes += entry[1].bytes;
  }
  if (batch.length) batches.push(batch);
  return batches;
}
