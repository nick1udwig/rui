// Match BrowserClient's scope-local incremental export without a second reducer.
export function exportState(session, previous) {
  if (!previous) return session.state();
  const known = new BigUint64Array(previous.records.size * 2);
  let index = 0;
  for (const [id, record] of previous.records) {
    known[index++] = id;
    known[index++] = Object.values(record)[0].version;
  }
  const changes = session.stateChanges(known);
  const records = new Map(previous.records);
  for (const id of changes.removed) records.delete(id);
  for (const [id, record] of changes.records) records.set(id, record);
  return { revision: changes.revision, records, retired: changes.retired };
}
