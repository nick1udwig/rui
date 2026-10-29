import test from 'node:test';
import assert from 'node:assert/strict';
import { CaptureCache } from '../capture-cache.mjs';
test('capture generations omit unchanged bodies and recover unknown clients', () => {
  const cache = new CaptureCache(), first = {nodes: []}, second = {nodes: []};
  assert.deepEqual(cache.reply(first, 0), {generation: 1, capture: first});
  assert.deepEqual(cache.reply(first, 1), {generation: 1});
  assert.deepEqual(cache.reply(second, 1), {generation: 2, capture: second});
  assert.deepEqual(cache.reply(second, 0), {generation: 2, capture: second});
});
