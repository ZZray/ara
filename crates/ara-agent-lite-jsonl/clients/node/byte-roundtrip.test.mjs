// Property 36: JSONL cross-language byte round trip (requirement 26.3), Node side.
//
// The actual sidecar (ARA_JSONL_TEST_BINARY, default
// $CARGO_TARGET_DIR/debug/ara-agent-lite-jsonl.exe) runs behind tests/frame_tap.mjs,
// which records raw frame bytes. A loopback provider owned by this test sends
// generated strings (Property 35 rules) in batches as `echo_bytes` tool calls;
// the Node client's tool callback records the decoded UTF-8 bytes and returns
// the strings unchanged. Checked per string (at least 100, plus the fixed
// sample): exact bytes after sidecar -> client and after client -> sidecar
// (the tool result sent upstream); every U+0000 is a \u0000 escape, no frame
// has a raw 00 byte, each frame is one complete line. Seed: ARA_BYTE_SEED (36).
// No actual model.
import test from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs, existsSync } from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runTask } from './client.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const TAP = path.resolve(here, '../../tests/frame_tap.mjs');
const BINARY = process.env.ARA_JSONL_TEST_BINARY ?? path.join(process.env.CARGO_TARGET_DIR ?? 'D:/workspace/env/cache/rust-target-c',
  'debug', `ara-agent-lite-jsonl${process.platform === 'win32' ? '.exe' : ''}`);
const SAMPLE = 'header\u0000中文😀';
const SAMPLE_HEX = '68656164657200e4b8ade69687f09f9880';
const CASES = 100;
const BATCH_STRINGS = 8; const BATCH_BYTES = 16 * 1024;
const range = (from, to) => Array.from({ length: to - from }, (_, i) => from + i);
const SPECIALS = [...range(0x00, 0x0a), 0x0b, 0x0c, ...range(0x0e, 0x20), 0x22, 0x5c, 0x7f, ...range(0x80, 0xa0),
  0x2028, 0x2029, 0xfeff, 0xfffd, 0xfffe, 0xffff, 0x10ffff];

function prng(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function generator(seed) {
  const random = prng(seed);
  const between = (from, to) => from + Math.floor(random() * (to - from));
  const scalar = () => {
    for (;;) {
      const roll = random();
      const point = roll < 0.3 ? between(0, 0x80) : roll < 0.5 ? between(0x80, 0x10000)
        : roll < 0.65 ? between(0x10000, 0x110000) : roll < 0.75 ? between(0x300, 0x370)
          : roll < 0.9 ? SPECIALS[between(0, SPECIALS.length)] : between(0, 0x110000);
      if (point !== 0x0a && point !== 0x0d && !(point >= 0xd800 && point <= 0xdfff)) return String.fromCodePoint(point);
    }
  };
  return () => {
    const length = between(4, 1025);
    const units = ['\u0000', String.fromCodePoint(between(0x10000, 0x110000)),
      'aeouAEOUnz'[between(0, 10)] + String.fromCodePoint(between(0x300, 0x370))];
    for (let i = 0; i < length - 4; i += 1) units.push(scalar());
    for (let i = units.length - 1; i > 0; i -= 1) { const j = between(0, i + 1); [units[i], units[j]] = [units[j], units[i]]; }
    return units.join('');
  };
}

function batches(strings) {
  const out = []; let current = []; let size = 0;
  for (const value of strings) {
    const length = Buffer.byteLength(value, 'utf8');
    if (current.length && (current.length === BATCH_STRINGS || size + length > BATCH_BYTES)) { out.push(current); current = []; size = 0; }
    current.push(value); size += length;
  }
  return current.length ? [...out, current] : out;
}

// Count \u0000 JSON escapes, scanning escapes exactly (an escaped backslash is not one).
function nulEscapes(raw) {
  let count = 0; let index = 0;
  while ((index = raw.indexOf(0x5c, index)) >= 0) {
    if (raw[index + 1] === 0x75) { count += raw.subarray(index + 2, index + 6).toString('latin1') === '0000'; index += 6; } else index += 2;
  }
  return count;
}

function firstDifference(expected, actual) {
  const left = Buffer.from(expected, 'utf8'); const right = Buffer.from(actual, 'utf8');
  let at = 0; while (at < left.length && at < right.length && left[at] === right[at]) at += 1;
  return `expected ${left.length} bytes, got ${right.length}; first difference at byte ${at}: expected `
    + `${left.subarray(Math.max(0, at - 8), at + 8).toString('hex')} got ${right.subarray(Math.max(0, at - 8), at + 8).toString('hex')}; `
    + `full expected ${left.toString('hex')}`;
}

async function provider(t, replies) {
  const requests = [];
  const server = http.createServer((request, response) => {
    if (request.method === 'GET' && request.url === '/v1/models') {
      response.writeHead(200, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify({ data: [{ id: 'jsonl-fixture', protocols: ['openai-completions'], context_window: 1000000,
        max_tokens: 4096, omp: { supportsTools: true, input: ['text'], reasoning: false } }] }));
      return;
    }
    const chunks = [];
    request.on('data', (chunk) => chunks.push(chunk));
    request.on('end', () => {
      const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      const reply = replies[requests.length] ?? { delta: { content: 'No further scripted reply.' }, finish: 'stop' };
      requests.push(body);
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      for (const [delta, finish] of [[{ role: 'assistant' }, null], [reply.delta, null], [{}, reply.finish]]) {
        response.write(`data: ${JSON.stringify({ id: 'controlled-only', object: 'chat.completion.chunk',
          choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`);
      }
      response.end('data: [DONE]\n\n');
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  return { endpoint: `http://127.0.0.1:${server.address().port}/v1`, requests };
}

function startFrame(root, endpoint, calls) {
  return { kind: 'start_task', version: 1,
    task: { task_id: 'byte-roundtrip', criteria_revision: 'c1', authorization_revision: 'a1', kind: 'Coding',
      evidence_dir: path.join(root, 'evidence'), max_runs: 1, max_calls: calls, deadline_unix_ms: Date.now() + 240000 },
    run: { run_id: 'run-1', objective: 'Echo every string with the Host tool.', cwd: root,
      session_path: path.join(root, 'session.json'), environment_state: path.join(root, 'environment'),
      device_fp: 'byte-roundtrip', session_key: 'byte/roundtrip', mode: 'execute', endpoint, model: 'jsonl-fixture',
      max_calls: calls, max_seconds: 180, max_output: 512, model_resolution: { kind: 'catalog' },
      tools: [{ name: 'echo_bytes', description: 'Host echo of the given strings.',
        parameters: { type: 'object', properties: { items: { type: 'array', items: { type: 'string' } } }, required: ['items'] } }] },
    cancel_grace_ms: 1000 };
}

async function roundTrip(t, strings) {
  assert.ok(existsSync(BINARY), `build the sidecar first: ${BINARY}`);
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'ara-node-bytes-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const groups = batches(strings);
  const upstream = await provider(t, [...groups.map((items, i) => ({ delta: { tool_calls: [{ index: 0, id: `op-${i}`, type: 'function',
    function: { name: 'echo_bytes', arguments: JSON.stringify({ items }) } }] }, finish: 'tool_calls' })),
  { delta: { content: 'All strings echoed.' }, finish: 'stop' }]);
  const log = path.join(root, 'tap.jsonl');
  const decoded = [];
  const result = await runTask({ binary: process.execPath, binaryArgs: [TAP, BINARY, log],
    start: startFrame(root, upstream.endpoint, groups.length + 1), modelToken: 'controlled-bytes-no-provider-grant',
    operationDirectory: path.join(root, 'ops'), callbacks: {
      tool: async (frame) => {
        const { items } = frame.arguments;
        decoded.push({ id: frame.native_call_id, bytes: items.map((item) => Buffer.from(item, 'utf8')) });
        return { value: { items }, is_error: false, settlement: 'Settled', operation_receipt: `echo-${frame.rpc_id}` };
      },
      verify: async (candidate) => ({ basis: candidate.basis, committed: true, settlement: 'Settled', verdict: 'Verified',
        evidence: ['controlled-review-only'] }),
      prepare: async () => { throw new Error('no continuation'); } } });
  assert.equal(result.outcome?.stop, 'AwaitingHostAcceptance', result.fault);
  // sidecar -> client
  assert.deepEqual(decoded.map((entry) => entry.id), groups.map((_, i) => `op-${i}`));
  groups.forEach((group, i) => {
    assert.equal(decoded[i].bytes.length, group.length);
    group.forEach((expected, j) => {
      const actual = decoded[i].bytes[j];
      assert.ok(actual.equals(Buffer.from(expected, 'utf8')), `Node client decoded: ${firstDifference(expected, actual.toString('utf8'))}`);
    });
  });
  // client -> sidecar: the ToolResult the sidecar sent upstream
  groups.forEach((group, i) => {
    const messages = upstream.requests[i + 1].messages.filter((message) => message.role === 'tool' && message.tool_call_id === `op-${i}`);
    assert.equal(messages.length, 1);
    const content = typeof messages[0].content === 'string' ? messages[0].content
      : messages[0].content.filter((part) => part.type === 'text').map((part) => part.text).join('');
    const returned = JSON.parse(content).items;
    assert.equal(returned.length, group.length);
    group.forEach((expected, j) => assert.ok(Buffer.from(returned[j], 'utf8').equals(Buffer.from(expected, 'utf8')),
      `Node client -> sidecar: ${firstDifference(expected, returned[j])}`));
  });
  // raw frames in both directions
  const frames = { sidecar: [], host: [] };
  for (const line of (await fs.readFile(log, 'utf8')).split('\n').filter(Boolean)) {
    const row = JSON.parse(line); const raw = Buffer.from(row.base64, 'base64');
    assert.ok(raw.at(-1) === 0x0a && raw.indexOf(0x0a) === raw.length - 1, 'one complete line per frame');
    assert.equal(raw.indexOf(0x00), -1, `raw 00 byte in a ${row.from} frame`);
    const frame = JSON.parse(raw.toString('utf8'));
    if (frame.kind === 'tool_call' || frame.kind === 'tool_reply') frames[row.from].push({ raw, frame });
  }
  assert.deepEqual(frames.sidecar.map(({ frame }) => frame.kind), groups.map(() => 'tool_call'));
  assert.deepEqual(frames.host.map(({ frame }) => frame.kind), groups.map(() => 'tool_reply'));
  groups.forEach((group, i) => {
    const nul = group.reduce((sum, value) => sum + value.split('\u0000').length - 1, 0);
    assert.deepEqual(frames.sidecar[i].frame.arguments.items, group, 'tool_call frame not truncated');
    assert.deepEqual(frames.host[i].frame.result.value.items, group, 'tool_reply frame not truncated');
    assert.equal(nulEscapes(frames.sidecar[i].raw), nul, 'each U+0000 is a \\u0000 escape in tool_call');
    assert.equal(nulEscapes(frames.host[i].raw), nul, 'each U+0000 is a \\u0000 escape in tool_reply');
  });
  return decoded;
}

test('generated strings round trip through the Node client and the actual sidecar', { timeout: 240000 }, async (t) => {
  const next = generator(Number(process.env.ARA_BYTE_SEED ?? 36));
  const strings = [SAMPLE, ...Array.from({ length: CASES }, next)];
  for (const value of strings.slice(1)) {
    const points = [...value].map((c) => c.codePointAt(0));
    assert.ok(points.length >= 4 && points.length <= 1024 && points.includes(0) && points.some((p) => p >= 0x10000)
      && points.some((p) => p >= 0x300 && p <= 0x36f) && !points.includes(0x0a) && !points.includes(0x0d));
  }
  assert.equal(Buffer.from(SAMPLE, 'utf8').toString('hex'), SAMPLE_HEX);
  await roundTrip(t, strings);
});

test('the sample round trips through the Node client as 17 bytes', { timeout: 120000 }, async (t) => {
  const decoded = await roundTrip(t, [SAMPLE]);
  assert.equal(decoded[0].bytes[0].toString('hex'), SAMPLE_HEX);
});
