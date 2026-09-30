// Fixed OMP/Bun byte oracle. Python exports exact source and unchanged tests.
import { createHash } from "node:crypto";
import * as fs from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const COMMIT = "596f2da7101178214aa27a753529d15e6b7ad91d";
if (Bun.version !== "1.4.0") throw new Error("Bun 1.4.0 is required");
if (process.argv.length !== 3) throw new Error("Pass source-manifest.json");
const manifestPath = resolve(process.argv[2]);
const runDir = dirname(manifestPath);
const root = join(runDir, "upstream");
const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");
const manifestBytes = await fs.readFile(manifestPath);
const manifest = JSON.parse(manifestBytes.toString("utf8"));
if (manifest.upstreamCommit !== COMMIT || manifest.bunVersion !== Bun.version) throw new Error("Source version mismatch");
if (manifest.runnerSha256 !== sha256(await fs.readFile(fileURLToPath(import.meta.url)))) {
  throw new Error("Runner hash mismatch");
}
for (const source of manifest.sources) {
  const bytes = await fs.readFile(join(root, source.path));
  if (sha256(bytes) !== source.sha256) throw new Error(`Source SHA-256 mismatch: ${source.path}`);
  const blob = createHash("sha1").update(`blob ${bytes.length}\0`).update(bytes).digest("hex");
  if (blob !== source.gitBlob) throw new Error(`Source Git blob mismatch: ${source.path}`);
}
for (const helper of manifest.helpers) {
  if (sha256(await fs.readFile(join(root, helper.path))) !== helper.sha256) {
    throw new Error(`Package alias mismatch: ${helper.path}`);
  }
}

const rpcPath = join(root, "packages", "coding-agent", "src", "modes", "rpc");
const frameModule = await import(pathToFileURL(join(rpcPath, "rpc-frame.ts")).href);
const inputModule = await import(pathToFileURL(join(rpcPath, "rpc-input.ts")).href);
const { encodeRpcFrame, RpcFrameEncoder, RpcFrameDecoder, MAX_RPC_FRAME_BYTES, MAX_RPC_REASSEMBLED_BYTES } = frameModule;
const { readRpcInputFrames } = inputModule;
const cases = [];
const captureError = error => ({ name: error?.name ?? null, message: String(error?.message ?? error) });
const fingerprint = bytes => ({ byteLength: bytes.length, sha256: sha256(bytes) });

// All physical bytes are checked; large payloads are recorded as per-line
// digests and previews instead of duplicating up to 90 MiB of base64 in JSON.
function describeLines(lines) {
  const aggregate = createHash("sha256");
  const physical = [];
  let totalBytes = 0;
  let firstPrefixBase64;
  let lastSuffixBase64;
  let singleBase64;
  for (const line of lines) {
    const bytes = Buffer.from(line, "utf8");
    aggregate.update(bytes);
    totalBytes += bytes.length;
    if (firstPrefixBase64 === undefined) firstPrefixBase64 = bytes.subarray(0, 160).toString("base64");
    lastSuffixBase64 = bytes.subarray(Math.max(0, bytes.length - 160)).toString("base64");
    physical.push(fingerprint(bytes));
    if (physical.length === 1 && bytes.length <= 4096) singleBase64 = bytes.toString("base64");
    if (physical.length > 1) singleBase64 = undefined;
    if (!line.endsWith("\n") || bytes.length > MAX_RPC_FRAME_BYTES) {
      throw new Error("Upstream emitted an invalid physical frame");
    }
    JSON.parse(line);
  }
  return { totalBytes, sha256: aggregate.digest("hex"), physical, firstPrefixBase64, lastSuffixBase64,
    ...(singleBase64 === undefined ? {} : { encodedBase64: singleBase64 }) };
}

function addEncode(id, recipe, makeFrame, version = 1) {
  try {
    const frame = makeFrame();
    const encoder = new RpcFrameEncoder();
    encoder.setProtocolVersion(version);
    const result = describeLines(encoder.encodeFrames(frame));
    const observations = id === "v1-utf16-split"
      ? { escapedLoneHighSurrogate: encodeRpcFrame(frame).includes("\\ud83d") }
      : undefined;
    cases.push({ kind: "encode", id, version, recipe, result, ...(observations ? { observations } : {}) });
  } catch (error) {
    cases.push({ kind: "encode", id, version, recipe, error: captureError(error) });
  }
}

function response(payload) {
  return { id: "req", type: "response", command: "get_state", success: true, data: { payload } };
}
const responseOverhead = Buffer.byteLength(JSON.stringify(response("")), "utf8");
addEncode("basic-v1", { kind: "literal", frame: response("ok") }, () => response("ok"));
addEncode("basic-v2", { kind: "literal", frame: response("ok") }, () => response("ok"), 2);
addEncode("v1-below-byte-ceiling-by-one", { kind: "response-repeat", char: "x", count: MAX_RPC_FRAME_BYTES - responseOverhead - 2 },
  () => response("x".repeat(MAX_RPC_FRAME_BYTES - responseOverhead - 2)));
addEncode("v1-at-byte-ceiling", { kind: "response-repeat", char: "x", count: MAX_RPC_FRAME_BYTES - responseOverhead - 1 },
  () => response("x".repeat(MAX_RPC_FRAME_BYTES - responseOverhead - 1)));
addEncode("v1-one-byte-over", { kind: "response-repeat", char: "x", count: MAX_RPC_FRAME_BYTES - responseOverhead },
  () => response("x".repeat(MAX_RPC_FRAME_BYTES - responseOverhead)));
addEncode("v1-multibyte-shrink", { kind: "event-repeat", char: "😀", count: 300000 },
  () => ({ type: "message_end", message: { role: "assistant", content: [{ type: "text", text: "😀".repeat(300000) }] } }));
addEncode("v1-utf16-split", { kind: "event-repeat", prefix: "A", char: "😀", count: 300000 },
  () => ({ type: "message_end", message: { role: "assistant", content: [{ type: "text", text: "A" + "😀".repeat(300000) }] } }));
addEncode("v1-lone-surrogate", { kind: "literal-json", json: '{"type":"message_end","message":"\\ud800"}' },
  () => JSON.parse('{"type":"message_end","message":"\\ud800"}'));
addEncode("v1-number-rounding", { kind: "literal-json", json: '{"type":"message_end","n":9007199254740993,"z":-0,"e":1e21}' },
  () => JSON.parse('{"type":"message_end","n":9007199254740993,"z":-0,"e":1e21}'));
addEncode("v1-integer-key-order", { kind: "literal-json", json: '{"type":"message_end","9":"n","2":"m","alpha":"a","1":"l"}' },
  () => JSON.parse('{"type":"message_end","9":"n","2":"m","alpha":"a","1":"l"}'));
addEncode("v2-at-byte-ceiling", { kind: "response-repeat", char: "x", count: MAX_RPC_FRAME_BYTES - responseOverhead - 1 },
  () => response("x".repeat(MAX_RPC_FRAME_BYTES - responseOverhead - 1)), 2);
addEncode("v2-below-byte-ceiling-by-one", { kind: "response-repeat", char: "x", count: MAX_RPC_FRAME_BYTES - responseOverhead - 2 },
  () => response("x".repeat(MAX_RPC_FRAME_BYTES - responseOverhead - 2)), 2);
addEncode("v2-one-byte-over", { kind: "response-repeat", char: "x", count: MAX_RPC_FRAME_BYTES - responseOverhead },
  () => response("x".repeat(MAX_RPC_FRAME_BYTES - responseOverhead)), 2);
addEncode("v2-below-reassembly-ceiling-by-one", { kind: "response-repeat", char: "x", count: MAX_RPC_REASSEMBLED_BYTES - responseOverhead - 1 },
  () => response("x".repeat(MAX_RPC_REASSEMBLED_BYTES - responseOverhead - 1)), 2);
addEncode("v2-at-reassembly-ceiling", { kind: "response-repeat", char: "x", count: MAX_RPC_REASSEMBLED_BYTES - responseOverhead },
  () => response("x".repeat(MAX_RPC_REASSEMBLED_BYTES - responseOverhead)), 2);
addEncode("v2-one-byte-over-reassembly", { kind: "response-repeat", char: "x", count: MAX_RPC_REASSEMBLED_BYTES - responseOverhead + 1 },
  () => response("x".repeat(MAX_RPC_REASSEMBLED_BYTES - responseOverhead + 1)), 2);

function addStateful(id, version) {
  const recipe = { kind: "stateful-mutation", messageCount: 20, messageChars: 65536,
    mutation: { firstTextPrefix: "changed" }, version };
  const messages = Array.from({ length: 20 }, (_, i) => ({ role: "assistant", content: [{ type: "text", text: `${i}-` + "x".repeat(65536) }] }));
  const encoder = new RpcFrameEncoder();
  encoder.setProtocolVersion(version);
  const start = describeLines(encoder.encodeFrames({ type: "agent_start" }));
  const streamed = messages.map(message => describeLines(encoder.encodeFrames({ type: "message_end", message })));
  messages[0].content[0].text = "changed" + messages[0].content[0].text;
  const terminal = describeLines(encoder.encodeFrames({ type: "agent_end", messages, willContinue: true }));
  const final = describeLines(encoder.encodeFrames({ type: "agent_end", messages }));
  cases.push({ kind: "stateful", id, recipe, result: { start, streamed, terminal, final } });
}
addStateful("v1-stateful-mutation", 1);
addStateful("v2-stateful-mutation", 2);

function addMatchedState(id, version) {
  const messages = Array.from({ length: 20 }, (_, i) => ({ role: "assistant",
    content: [{ type: "text", text: `${i}-` + "x".repeat(65536) }] }));
  const encoder = new RpcFrameEncoder();
  encoder.setProtocolVersion(version);
  const start = describeLines(encoder.encodeFrames({ type: "agent_start" }));
  const streamed = messages.map(message => describeLines(encoder.encodeFrames({ type: "message_end", message })));
  const continuingFrame = { type: "agent_end", messages, willContinue: true };
  const continuing = describeLines(encoder.encodeFrames(continuingFrame));
  const final = describeLines(encoder.encodeFrames({ type: "agent_end", messages }));
  const afterFinalReset = describeLines(encoder.encodeFrames({ type: "agent_end", messages }));
  const newStart = describeLines(encoder.encodeFrames({ type: "agent_start" }));
  const afterStartReset = describeLines(encoder.encodeFrames({ type: "agent_end", messages }));
  const observations = { continuingCompact: continuing.totalBytes < 1024,
    finalCompact: final.totalBytes < 1024,
    afterFinalNotCompact: afterFinalReset.totalBytes > 1024,
    afterStartNotCompact: afterStartReset.totalBytes > 1024 };
  if (Object.values(observations).some(value => !value)) throw new Error(`${id}: snapshot/reset semantics changed`);
  cases.push({ kind: "stateful", id, recipe: { kind: "stateful-prefix", version,
    messageCount: 20, messageChars: 65536, sequence: ["start", "stream", "continue", "final", "repeat", "start", "repeat"] },
  observations, result: { start, streamed, continuing, final, afterFinalReset, newStart, afterStartReset } });
}
addMatchedState("v1-stateful-matched-prefix", 1);
addMatchedState("v2-stateful-matched-prefix", 2);
function addShrunkState(id, version) {
  const message = { role: "assistant", content: [{ type: "text", text: "😀".repeat(300000) }] };
  const encoder = new RpcFrameEncoder();
  encoder.setProtocolVersion(version);
  const start = describeLines(encoder.encodeFrames({ type: "agent_start" }));
  const streamed = describeLines(encoder.encodeFrames({ type: "message_end", message }));
  const terminal = describeLines(encoder.encodeFrames({ type: "agent_end", messages: [message] }));
  const observations = { terminalCompacted: terminal.totalBytes < 1024 };
  if (observations.terminalCompacted !== (version === 2)) throw new Error(`${id}: streamed snapshot shape changed`);
  cases.push({ kind: "stateful", id, recipe: { kind: "stateful-shrunk-message", version,
    char: "😀", count: 300000, sequence: ["start", "stream", "terminal"] },
    observations, result: { start, streamed, terminal } });
}
addShrunkState("v1-stateful-shrunk-message", 1);
addShrunkState("v2-stateful-shrunk-message", 2);

const arrayOf = (count, itemChars) => Array.from({ length: count }, () => "x".repeat(itemChars));
function keyFields(fieldCount, keyPadding) {
  return Object.fromEntries(Array.from({ length: fieldCount }, (_, i) => [`k${i}-` + "x".repeat(keyPadding), 0]));
}
function addKeyShrink(pass, fieldCount, keyPadding, retained) {
  const id = `shrink-pass-${pass}`;
  const recipe = { kind: "event-key-fields", fieldCount, keyPadding, value: 0 };
  const frame = { type: "tool_execution_end", details: keyFields(fieldCount, keyPadding) };
  const encoded = encodeRpcFrame(frame);
  const decoded = JSON.parse(encoded);
  const keyCount = Object.keys(decoded.details ?? {}).length - 1;
  const elided = decoded.details?.rpcFrameElidedKeys;
  if (keyCount !== retained || elided !== fieldCount - retained) {
    throw new Error(`${id}: expected ${retained} retained/${fieldCount - retained} elided, got ${keyCount}/${elided}`);
  }
  cases.push({ kind: "encode", id, version: 1, recipe,
    observations: { retainedKeys: keyCount, elidedKeys: elided }, result: describeLines([encoded]) });
}
[[600, 2000, 512], [513, 3000, 256], [257, 6000, 128], [129, 12000, 64],
  [65, 24000, 32], [33, 48000, 16], [17, 96000, 8]]
  .forEach(([fieldCount, keyPadding, retained], i) => addKeyShrink(i + 1, fieldCount, keyPadding, retained));
const unshrinkable = { type: "tool_execution_end", details: { ["k" + "x".repeat(MAX_RPC_FRAME_BYTES + 1)]: 0 } };
const unshrinkableEncoded = encodeRpcFrame(unshrinkable);
cases.push({ kind: "encode", id: "v1-unshrinkable-key", version: 1,
  recipe: { kind: "event-one-huge-key", keyPadding: MAX_RPC_FRAME_BYTES + 1 },
  observations: { resultType: JSON.parse(unshrinkableEncoded).type },
  result: describeLines([unshrinkableEncoded]) });

// Object.entries key order and the ordinary-object assignment effects inside
// shrinkValue are observable only after an oversized frame chooses a pass.
const collisionPayload = Object.fromEntries([
  ["rpcFrameElidedKeys", "original"], ["__proto__", "own-json-key"],
  ...Array.from({ length: 600 }, (_, i) => [`k${i}`, "x".repeat(2000)]),
]);
const collisionFrame = JSON.parse(JSON.stringify({ type: "message_end", payload: collisionPayload }));
const collisionEncoded = encodeRpcFrame(collisionFrame);
const collisionDecoded = JSON.parse(collisionEncoded);
cases.push({ kind: "encode", id: "shrink-object-special-keys", version: 1,
  recipe: { kind: "event-object-special-keys", keys: 600, itemChars: 2000,
    firstKeys: ["rpcFrameElidedKeys", "__proto__"] },
  observations: { elidedKeys: collisionDecoded.payload?.rpcFrameElidedKeys,
    hasOwnProto: Object.hasOwn(collisionDecoded.payload ?? {}, "__proto__") },
  result: describeLines([collisionEncoded]) });
const arrayRetain = { type: "message_end", payload: arrayOf(600, 2000) };
const arrayEncoded = encodeRpcFrame(arrayRetain);
const arrayDecoded = JSON.parse(arrayEncoded);
cases.push({ kind: "encode", id: "shrink-array-retention", version: 1,
  recipe: { kind: "event-array-repeat", count: 600, itemChars: 2000 },
  observations: { retained: arrayDecoded.payload?.length,
    terminalElision: arrayDecoded.payload?.at(-1) }, result: describeLines([arrayEncoded]) });

function addDecode(id, recipe, frames) {
  const decoder = new RpcFrameDecoder();
  const outputs = [];
  try {
    for (const frame of frames) {
      const value = decoder.push(frame);
      if (value !== undefined) {
        const json = JSON.stringify(value);
        outputs.push({ ...fingerprint(Buffer.from(json, "utf8")), ...(json.length < 4096 ? { json } : {}) });
      }
    }
    cases.push({ kind: "decode", id, recipe, result: outputs });
  } catch (error) {
    cases.push({ kind: "decode", id, recipe, outputsBeforeError: outputs, error: captureError(error) });
  }
}
function addDecodeTrace(id, recipe, frames) {
  const decoder = new RpcFrameDecoder();
  const trace = [];
  for (const frame of frames) {
    try {
      const value = decoder.push(frame);
      if (value === undefined) trace.push({ event: "pending" });
      else {
        const json = JSON.stringify(value);
        trace.push({ event: "frame", ...fingerprint(Buffer.from(json, "utf8")),
          ...(json.length < 4096 ? { json } : {}) });
      }
    } catch (error) { trace.push({ event: "error", ...captureError(error) }); }
  }
  cases.push({ kind: "decode-trace", id, recipe, result: trace });
}
const chunk = (fields = {}) => ({ type: "rpc_chunk", chunkId: "c", index: 0, count: 2,
  byteLength: MAX_RPC_FRAME_BYTES, data: Buffer.from("a").toString("base64"), ...fields });
addDecode("v1-object", { kind: "literal", frames: [{ type: "ready" }] }, [{ type: "ready" }]);
addDecode("v1-nonobject", { kind: "literal", frames: [null] }, [null]);
const validSource = response("x".repeat(MAX_RPC_FRAME_BYTES - responseOverhead));
const validEncoder = new RpcFrameEncoder();
validEncoder.setProtocolVersion(2);
const validChunks = Array.from(validEncoder.encodeFrames(validSource), line => JSON.parse(line));
addDecode("v2-valid-chunks", { kind: "encoded-response-repeat", char: "x",
  count: MAX_RPC_FRAME_BYTES - responseOverhead }, validChunks);
const variableBytes = Buffer.from(JSON.stringify(validSource), "utf8");
const variableSizes = [100000, 200000, 250000, 250000, MAX_RPC_FRAME_BYTES - 800000];
let variableOffset = 0;
const variableChunks = variableSizes.map((size, index) => {
  const data = variableBytes.subarray(variableOffset, variableOffset + size).toString("base64");
  variableOffset += size;
  return chunk({ index, count: variableSizes.length, data });
});
addDecode("chunk-variable-sizes", { kind: "logical-response-variable-chunks", char: "x",
  count: MAX_RPC_FRAME_BYTES - responseOverhead, sizes: variableSizes }, variableChunks);
addDecode("chunk-noncanonical-base64", { kind: "literal", frames: [chunk({ data: "YR==" })] }, [chunk({ data: "YR==" })]);
addDecode("chunk-wrong-start", { kind: "literal", frames: [chunk({ index: 1 })] }, [chunk({ index: 1 })]);
addDecode("chunk-interrupted", { kind: "literal", frames: [chunk(), { type: "response" }] }, [chunk(), { type: "response" }]);
addDecode("chunk-metadata-mismatch", { kind: "literal", frames: [chunk(), chunk({ index: 1, count: 3 })] },
  [chunk(), chunk({ index: 1, count: 3 })]);
const tooManyBytes = Array.from({ length: 5 }, (_, index) => chunk({ index, count: 5,
  data: Buffer.alloc(256 * 1024).toString("base64") }));
addDecode("chunk-declared-length-overflow", { kind: "chunk-repeat", byte: 0, count: 5,
  payloadBytes: 256 * 1024, declaredBytes: MAX_RPC_FRAME_BYTES }, tooManyBytes);
const shortChunks = Array.from({ length: 4 }, (_, index) => chunk({ index, count: 4,
  data: Buffer.alloc(index === 3 ? 256 * 1024 - 1 : 256 * 1024).toString("base64") }));
addDecode("chunk-declared-length-short", { kind: "chunk-repeat", byte: 0, count: 4,
  payloadBytes: 256 * 1024, lastPayloadBytes: 256 * 1024 - 1, declaredBytes: MAX_RPC_FRAME_BYTES }, shortChunks);
addDecodeTrace("chunk-short-then-ready", { kind: "chunk-repeat-then-ready", byte: 0, count: 4,
  payloadBytes: 256 * 1024, lastPayloadBytes: 256 * 1024 - 1, declaredBytes: MAX_RPC_FRAME_BYTES },
  [...shortChunks, { type: "ready" }]);
const invalidUtf8 = Buffer.alloc(MAX_RPC_FRAME_BYTES, 0x20);
invalidUtf8[0] = 0xff;
const invalidUtf8Chunks = Array.from({ length: 4 }, (_, index) => chunk({ index, count: 4,
  data: invalidUtf8.subarray(index * 256 * 1024, (index + 1) * 256 * 1024).toString("base64") }));
addDecode("chunk-fatal-utf8", { kind: "bytes-repeat", byte: 32, count: MAX_RPC_FRAME_BYTES,
  firstByte: 255, chunks: 4, payloadBytes: 256 * 1024 }, invalidUtf8Chunks);
addDecodeTrace("chunk-fatal-utf8-then-ready", { kind: "bytes-repeat-then-ready", byte: 32,
  count: MAX_RPC_FRAME_BYTES, firstByte: 255, chunks: 4, payloadBytes: 256 * 1024 },
  [...invalidUtf8Chunks, { type: "ready" }]);
for (const [id, overrides] of [
  ["empty-id", { chunkId: "" }], ["long-id", { chunkId: "x".repeat(129) }],
  ["fractional-index", { index: 0.5 }], ["unsafe-index", { index: Number.MAX_SAFE_INTEGER + 1 }],
  ["negative-index", { index: -1 }], ["one-count", { count: 1 }], ["large-count", { count: 257 }],
  ["short-byte-length", { byteLength: MAX_RPC_FRAME_BYTES - 1 }],
  ["long-byte-length", { byteLength: MAX_RPC_REASSEMBLED_BYTES + 1 }],
  ["metadata-before-data", { count: 1, data: "!" }], ["empty-data", { data: "" }],
  ["invalid-data", { data: "!" }],
]) addDecode(`chunk-${id}`, { kind: "chunk-overrides", overrides }, [chunk(overrides)]);
addDecode("chunk-payload-over-limit", { kind: "chunk-payload-repeat", byte: 0, count: 256 * 1024 + 1 },
  [chunk({ data: Buffer.alloc(256 * 1024 + 1).toString("base64") })]);
addDecodeTrace("chunk-retry-after-bad-data", { kind: "encoded-response-with-fault",
  char: "x", count: MAX_RPC_FRAME_BYTES - responseOverhead, faultAt: 1, faultData: "YR==" },
  [validChunks[0], { ...validChunks[1], data: "YR==" }, ...validChunks.slice(1)]);
addDecodeTrace("chunk-retry-after-interruption", { kind: "encoded-response-with-interruption",
  char: "x", count: MAX_RPC_FRAME_BYTES - responseOverhead, afterIndex: 0, interrupt: { type: "ready" } },
  [validChunks[0], { type: "ready" }, ...validChunks.slice(1)]);
function chunkLogicalBytes(text) {
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length !== MAX_RPC_FRAME_BYTES) throw new Error("bad logical error fixture length");
  return Array.from({ length: 4 }, (_, index) => chunk({ index, count: 4,
    data: bytes.subarray(index * 256 * 1024, (index + 1) * 256 * 1024).toString("base64") }));
}
addDecodeTrace("chunk-malformed-final-json", { kind: "chunked-logical-bytes", prefix: "{", filler: " ",
  totalBytes: MAX_RPC_FRAME_BYTES, after: { type: "ready" } },
  [...chunkLogicalBytes("{" + " ".repeat(MAX_RPC_FRAME_BYTES - 1)), { type: "ready" }]);
addDecodeTrace("chunk-final-nonobject", { kind: "chunked-logical-bytes", prefix: "null", filler: " ",
  totalBytes: MAX_RPC_FRAME_BYTES, after: { type: "ready" } },
  [...chunkLogicalBytes("null" + " ".repeat(MAX_RPC_FRAME_BYTES - 4)), { type: "ready" }]);

async function addInput(id, recipe, chunks) {
  const frames = [];
  const errors = [];
  const stream = new ReadableStream({ start(controller) {
    for (const bytes of chunks) controller.enqueue(bytes);
    controller.close();
  } });
  await readRpcInputFrames(stream, value => {
    const json = JSON.stringify(value);
    const bytes = Buffer.from(json ?? "undefined", "utf8");
    frames.push({ ...fingerprint(bytes), ...(bytes.length < 4096 ? { json } : {}),
      ...(typeof value === "number" ? { rawNumber: { negativeZero: Object.is(value, -0),
        finite: Number.isFinite(value), display: String(value) } } : {}) });
  }, message => errors.push(message));
  cases.push({ kind: "input", id, recipe, result: { frames, errors } });
}
const u8 = text => Buffer.from(text, "utf8");
await addInput("malformed-continues", { kind: "utf8", text: 'bad\n{"type":"get_state","id":"ok"}\n' },
  [u8('bad\n{"type":"get_state","id":"ok"}\n')]);
const split = u8('{"type":"prompt","message":"😀"}\n');
const emojiAt = split.indexOf(Buffer.from("😀", "utf8"));
await addInput("fragmented-utf8", { kind: "utf8-split", text: '{"type":"prompt","message":"😀"}\n',
  splitAfterBytes: [emojiAt + 1, emojiAt + 3] },
  [split.subarray(0, emojiAt + 1), split.subarray(emojiAt + 1, emojiAt + 3), split.subarray(emojiAt + 3)]);
await addInput("invalid-utf8", { kind: "hex", chunks: [Buffer.concat([u8('{"type":"prompt","message":"'), Buffer.from([0xff]), u8('"}\n')]).toString("hex")] },
  [Buffer.concat([u8('{"type":"prompt","message":"'), Buffer.from([0xff]), u8('"}\n')])]);
await addInput("eof-tail", { kind: "utf8", text: '{"type":"get_state","id":"tail"}' },
  [u8('{"type":"get_state","id":"tail"}')]);
await addInput("lone-surrogate-input", { kind: "utf8", text: '{"type":"prompt","message":"\\ud800"}\n' },
  [u8('{"type":"prompt","message":"\\ud800"}\n')]);
await addInput("oversized-input-no-cap", { kind: "utf8-repeat", prefix: '{"type":"prompt","message":"',
  char: "x", count: MAX_RPC_FRAME_BYTES + 1, suffix: '"}\n' },
  [u8('{"type":"prompt","message":"' + "x".repeat(MAX_RPC_FRAME_BYTES + 1) + '"}\n')]);
await addInput("raw-numeric-input", { kind: "utf8", text: '-0\n1e400\n9007199254740993\n' },
  [u8('-0\n1e400\n9007199254740993\n')]);
await addInput("crlf-bom-js-trim-and-nel", { kind: "utf8", text: '\uFEFF \u2028{"type":"get_state"}\u2029\r\n\u0085{}\n' },
  [u8('\uFEFF \u2028{"type":"get_state"}\u2029\r\n\u0085{}\n')]);
await addInput("malformed-eof-tail", { kind: "utf8", text: '{"type":' }, [u8('{"type":')]);
for (const depth of [128, 512]) {
  const text = "[".repeat(depth) + "0" + "]".repeat(depth);
  await addInput(`nested-input-depth-${depth}`, { kind: "nested-array", depth, leaf: 0, finalNewline: false }, [u8(text)]);
}
{
  const frames = [];
  const errors = [];
  let reads = 0;
  const stream = new ReadableStream({ pull(controller) {
    if (reads++ === 0) controller.enqueue(u8('{"type":"get_state"}\n'));
    else controller.error(new Error("injected read failure"));
  } });
  let thrown = null;
  try { await readRpcInputFrames(stream, value => frames.push(JSON.stringify(value)), message => errors.push(message)); }
  catch (error) { thrown = captureError(error); }
  if (!thrown) throw new Error("stream read failure was not propagated");
  cases.push({ kind: "input-error", id: "input-read-failure",
    recipe: { kind: "stream-error", priorLine: '{"type":"get_state"}\n', message: "injected read failure" },
    result: { frames, errors, thrown } });
}

const output = { upstreamCommit: COMMIT, bunVersion: Bun.version, sourceManifestSha256: sha256(manifestBytes),
  limits: { physical: MAX_RPC_FRAME_BYTES, reassembled: MAX_RPC_REASSEMBLED_BYTES }, cases };
if (cases.some(item => item.kind === "encode" && item.error)) throw new Error("Unexpected fixed-source encode failure");
if (!cases.find(item => item.id === "v1-utf16-split")?.observations?.escapedLoneHighSurrogate) {
  throw new Error("The UTF-16 cut fixture did not split a surrogate pair");
}
const byId = new Map(cases.map(item => [item.id, item]));
if (byId.size !== cases.length) throw new Error("Duplicate oracle case ID");
if (byId.get("shrink-object-special-keys")?.observations?.elidedKeys !== 90 ||
    byId.get("shrink-object-special-keys")?.observations?.hasOwnProto !== false) {
  throw new Error("Special-key shrink behavior changed");
}
if (byId.get("shrink-array-retention")?.observations?.retained !== 513 ||
    byId.get("shrink-array-retention")?.observations?.terminalElision !== "…[88 items elided for RPC frame]") {
  throw new Error("Array shrink behavior changed");
}
for (const id of ["chunk-retry-after-bad-data", "chunk-retry-after-interruption"]) {
  if (byId.get(id)?.result?.map(item => item.event).join(",") !== "pending,error,pending,pending,frame") {
    throw new Error(`${id}: decoder post-error state changed`);
  }
}
for (const id of ["chunk-malformed-final-json", "chunk-final-nonobject"]) {
  if (byId.get(id)?.result?.map(item => item.event).join(",") !== "pending,pending,pending,error,frame") {
    throw new Error(`${id}: decoder did not reset after completed logical frame error`);
  }
}
if (byId.get("chunk-fatal-utf8-then-ready")?.result?.map(item => item.event).join(",") !==
    "pending,pending,pending,error,frame") {
  throw new Error("Fatal UTF-8 must clear the completed chunk sequence");
}
if (byId.get("chunk-short-then-ready")?.result?.map(item => item.event).join(",") !==
    "pending,pending,pending,error,error") {
  throw new Error("Short completed sequence must retain pending state after length error");
}
if (byId.get("chunk-variable-sizes")?.result?.length !== 1) {
  throw new Error("Variable-size valid chunks were not reassembled");
}
const rawNumbers = byId.get("raw-numeric-input")?.result?.frames?.map(item => item.rawNumber);
if (rawNumbers?.length !== 3 || !rawNumbers[0].negativeZero || rawNumbers[1].finite ||
    rawNumbers[1].display !== "Infinity" || rawNumbers[2].display !== "9007199254740992") {
  throw new Error("Raw JavaScript number semantics changed");
}
if (byId.get("input-read-failure")?.result?.frames?.length !== 1 ||
    byId.get("input-read-failure")?.result?.thrown?.message !== "injected read failure") {
  throw new Error("Input stream failure handling changed");
}
await fs.writeFile(join(runDir, "oracle.json"), JSON.stringify(output, null, 2) + "\n");
process.stdout.write(JSON.stringify({ cases: cases.length, sourceManifestSha256: output.sourceManifestSha256 }) + "\n");
