// Property 32 (23.3, 23.7): Node client parsing of `unknown_effects` records.
// Pure function; seeded generator (no third-party dependency), 256 cases each.
import test from 'node:test';
import assert from 'node:assert/strict';
import { parseUnknownEffects } from './client.mjs';

const FIELDS = ['run_id', 'native_call_id', 'tool', 'receipt_ref'];
const CASES = 256;

function generator(seed) {
  let state = seed >>> 0;
  const next = () => { state = (state + 0x6D2B79F5) >>> 0; let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
  const int = (max) => Math.floor(next() * max);
  const text = () => Array.from({ length: 1 + int(16) }, () => String.fromCodePoint(32 + int(0x2FF))).join('');
  const hex = () => Array.from({ length: 64 }, () => '0123456789abcdef'[int(16)]).join('');
  const receipt = () => {
    const evidence = int(2) ? { kind: 'journal', session_path: text(), raw_index: int(10000), message_sha256: hex() }
      : { kind: 'event', events_path: text(), seq: int(10000) };
    return int(2) ? { evidence, host_receipt: { path: text(), sha256: hex() } } : { evidence };
  };
  const record = () => ({ run_id: text(), native_call_id: text(), tool: text(), receipt_ref: receipt() });
  return { int, record, list: (min) => Array.from({ length: min + int(6) }, record) };
}

test('valid records keep original order and exact values, frozen', () => {
  for (let seed = 1; seed <= CASES; seed += 1) {
    const g = generator(seed);
    const items = g.list(0);
    const original = structuredClone(items);
    const parsed = parseUnknownEffects({ task_id: 't', unknown_effects: items });
    assert.ok(Object.isFrozen(parsed));
    assert.equal(parsed.length, items.length);
    parsed.forEach((effect, index) => {
      assert.ok(Object.isFrozen(effect));
      assert.deepEqual({ ...effect }, original[index]);
    });
    assert.deepEqual(items, original, 'parsing never changes the original outcome');
  }
});

test('an invalid record names its field and builds nothing', () => {
  const wrong = [null, 7, [], '', {}];
  for (let seed = 1; seed <= CASES; seed += 1) {
    const g = generator(seed + 10_000);
    const items = g.list(1);
    const index = g.int(items.length);
    const field = FIELDS[g.int(FIELDS.length)];
    if (g.int(2)) delete items[index][field];
    else items[index][field] = field === 'receipt_ref' && g.int(2) ? { evidence: {} } : wrong[g.int(wrong.length)];
    assert.throws(() => parseUnknownEffects({ unknown_effects: items }),
      (error) => error.field === field && error.message === `invalid unknown effect record: ${field}`);
  }
});

test('absent list is empty; a non-list or an extra key is refused', () => {
  assert.deepEqual(parseUnknownEffects({ task_id: 't' }), []);
  assert.throws(() => parseUnknownEffects({ unknown_effects: {} }), (error) => error.field === 'unknown_effects');
  assert.throws(() => parseUnknownEffects({ unknown_effects: [{ run_id: 'r', native_call_id: 'c', tool: 't',
    receipt_ref: { evidence: { kind: 'event' } }, extra: 1 }] }), (error) => error.field === 'extra');
});
