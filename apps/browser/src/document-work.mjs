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
