import test from 'node:test';
import assert from 'node:assert/strict';
import { resourceReply } from '../document-resources.mjs';
import { resourceBatches } from '../src/document-work.mjs';

test('resource batches preserve single-key reads and bound count and bytes', () => {
  const first = 'a'.repeat(64), second = 'b'.repeat(64);
  const source = { resource: key => ({ [first]: 'image', [second]: 'style' })[key] };
  assert.deepEqual(resourceReply(source, { key: first }), { body: 'image' });
  assert.deepEqual(resourceReply(source, { keys: [first, second] }), { bodies: { [first]: 'image', [second]: 'style' } });
  for (const request of [
    { keys: [] }, { keys: null }, { keys: first }, { keys: [first, first] },
    { keys: [first], key: second }, { keys: ['../private'] },
    { keys: ['c'.repeat(64)] }, { keys: Array.from({ length: 33 }, (_, index) => index.toString(16).padStart(64, '0')) },
  ]) assert.throws(() => resourceReply(source, request));
  assert.throws(() => resourceReply({ resource: () => 'é'.repeat(3 * 1024 * 1024) }, { keys: [first, second] }), /response limit/);
});

test('download planning groups small resources and isolates large resources', () => {
  const entries = Array.from({ length: 80 }, (_, index) => [String(index), { bytes: 100 }]);
  assert.deepEqual(resourceBatches(entries).map(batch => batch.length), [32, 32, 16]);
  const large = [['one', { bytes: 1024 * 1024 }], ['two', { bytes: 1024 * 1024 }], ['three', { bytes: 3 * 1024 * 1024 }], ['four', { bytes: 10 }]];
  assert.deepEqual(resourceBatches(large).map(batch => batch.map(([key]) => key)), [['one', 'two'], ['three'], ['four']]);
});
