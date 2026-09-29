// The Python launcher exports the exact fixed Git blobs and their license.
// This runner imports those unchanged modules; mocks guard dead dependencies.
import { mock } from "bun:test";
import { createHash } from "node:crypto";
import * as fs from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const UPSTREAM_COMMIT = "596f2da7101178214aa27a753529d15e6b7ad91d";
const NATIVE_SHA256 = "fd757d36c44b8fa4cb184adc979f39b6aedabf8341d5a5316bf36f3c3949aa20";
if (Bun.version !== "1.4.0") throw new Error("Run this oracle with Bun 1.4.0");
if (process.argv.length !== 3) throw new Error("Pass the exported source-manifest.json path");
const manifestPath = resolve(process.argv[2]);
const runDir = dirname(manifestPath);
const manifest = JSON.parse(await fs.readFile(manifestPath, "utf8"));
const digest = bytes => createHash("sha256").update(bytes).digest("hex");
if (manifest.upstreamCommit !== UPSTREAM_COMMIT) throw new Error("Fixed upstream commit mismatch");
if (manifest.runnerSha256 !== digest(await fs.readFile(fileURLToPath(import.meta.url)))) {
  throw new Error("Runner bytes differ from the launch receipt");
}
const sourceRoot = join(runDir, "upstream", "packages", "coding-agent", "src");
async function verifySourceBytes() {
  for (const source of [...manifest.sourceManifest, manifest.license]) {
    const bytes = await fs.readFile(join(runDir, "upstream", source.path));
    if (digest(bytes) !== source.sha256) throw new Error(`Executed source hash mismatch: ${source.path}`);
    const gitBlob = createHash("sha1").update(`blob ${bytes.length}\0`).update(bytes).digest("hex");
    if (gitBlob !== source.gitBlob) throw new Error(`Executed Git blob mismatch: ${source.path}`);
  }
}
await verifySourceBytes();
if (manifest.native.sha256 !== NATIVE_SHA256 || digest(await fs.readFile(manifest.native.path)) !== NATIVE_SHA256) {
  throw new Error("Native binary hash mismatch");
}
const native = createRequire(import.meta.url)(manifest.native.path);
if (!native.__piNativesV18_1_8 || typeof native.enclosingBlockBoundaries !== "function") {
  throw new Error("Native marker or enclosingBlockBoundaries export is unavailable");
}

const mockCounts = {};
function trap(name) {
  mockCounts[name] = 0;
  return function () {
    mockCounts[name]++;
    throw new Error(`Immutable renderer unexpectedly invoked dead dependency: ${name}`);
  };
}
function propertyTrap(name) {
  mockCounts[name] = 0;
  return new Proxy({}, {
    get(_target, property) {
      mockCounts[name]++;
      throw new Error(`Immutable renderer unexpectedly read dead dependency: ${name}.${String(property)}`);
    },
  });
}
async function mockLocal(path, exports) {
  // Bun must resolve relative module paths before applying a mock. These
  // sentinels are separate from the twelve upstream blobs and throw if the
  // mock is ever bypassed; they implement no upstream behavior.
  const target = join(sourceRoot, path);
  await fs.mkdir(dirname(target), { recursive: true });
  await fs.writeFile(target, `throw new Error(${JSON.stringify(`Dead dependency loaded without mock: ${path}`)});\n`);
  mock.module(target, () => exports);
}
const utilityNames = [
  "hasFsCode", "isEnoent", "isEnotdir", "isWsl", "stripWindowsExtendedLengthPathPrefix",
  "windowsPathToWslMount", "isRecord", "formatBytes", "materializeString", "sanitizeText",
];
const utilities = Object.fromEntries(utilityNames.map(name => [name, trap(`pi-utils.${name}`)]));
utilities.logger = propertyTrap("pi-utils.logger");
utilities.$env = propertyTrap("pi-utils.$env");
utilities.$flag = trap("pi-utils.$flag");
mock.module("@oh-my-pi/pi-utils", () => utilities);
mock.module("@oh-my-pi/pi-catalog/identity", () => ({ classifyModel: trap("catalog.classifyModel") }));
await mockLocal("edit/store.ts", { getEditStore: trap("edit/store.getEditStore") });
await mockLocal("edit/normalize.ts", { normalizeToLF: trap("edit/normalize.normalizeToLF") });
await mockLocal("modes/theme/theme.ts", { isMarkdownPath: trap("theme.isMarkdownPath") });
await mockLocal("internal-urls/index.ts", { InternalUrlRouter: trap("InternalUrlRouter") });
await mockLocal("tools/render-utils.ts", {
  formatBytes: trap("render-utils.formatBytes"), shortenPath: trap("render-utils.shortenPath"),
  wrapBrackets: trap("render-utils.wrapBrackets"),
});
await mockLocal("config/settings.ts", { getDefault: trap("settings.getDefault") });
await mockLocal("lsp/utils.ts", { formatGroupedDiagnosticMessages: trap("lsp.formatGroupedDiagnosticMessages") });
await mockLocal("utils/sixel.ts", { sanitizeWithOptionalSixelPassthrough: trap("sixel.sanitizeWithOptionalSixelPassthrough") });
let nativeCalls = 0;
const nativeCallReceipts = [];
mock.module("@oh-my-pi/pi-natives", () => ({
  enclosingBlockBoundaries: (...args) => {
    nativeCalls++;
    const result = native.enclosingBlockBoundaries(...args);
    const request = args[0];
    nativeCallReceipts.push({
      path: request.path ?? null,
      lang: request.lang ?? null,
      codeSha256: digest(Buffer.from(request.code, "utf8")),
      ranges: request.ranges,
      boundaries: result,
    });
    return result;
  },
  glob: trap("pi-natives.glob"),
  hashlineFileHash: trap("pi-natives.hashlineFileHash"),
  hashlineFormatHeader: trap("pi-natives.hashlineFormatHeader"),
  hashlineFormatNumberedLines: trap("pi-natives.hashlineFormatNumberedLines"),
  hashlineStripPrefixes: trap("pi-natives.hashlineStripPrefixes"),
}));

const moduleAt = path => import(pathToFileURL(join(sourceRoot, path)).href);
const { parseSel } = await moduleAt("tools/read-selector.ts");
const { buildInMemorySelectorResult } = await moduleAt("tools/read-format.ts");
const { buildDirectoryResource } = await moduleAt("internal-urls/filesystem-resource.ts");
const plain = Array.from({ length: 12 }, (_, index) => `line${index + 1}`).join("\n") + "\n";
const fixtures = [
  { name: "plain.txt", kind: "file", text: plain },
  { name: "empty.txt", kind: "file", text: "" },
  { name: "crlf.txt", kind: "file", text: plain.replaceAll("\n", "\r\n") },
  { name: "blocks.ts", kind: "file", text: "function one() {\n  return 1;\n}\nfunction two() {\n  return 2;\n}\n" },
  { name: "listing", kind: "directory", dirs: ["alpha-dir", "z-dir"], files: Array.from({ length: 10 }, (_, index) => `file-${String(index + 1).padStart(2, "0")}.txt`) },
];
const fixtureRoot = join(runDir, "fixtures");
await fs.mkdir(fixtureRoot);
const resources = new Map();
for (const fixture of fixtures) {
  const sourcePath = join(fixtureRoot, fixture.name);
  if (fixture.kind === "file") {
    await fs.writeFile(sourcePath, fixture.text, "utf8");
    const content = await Bun.file(sourcePath).text();
    if (content !== fixture.text) throw new Error(`Fixture bytes changed: ${fixture.name}`);
    resources.set(fixture.name, { content, sourcePath, immutable: true });
  } else {
    await fs.mkdir(sourcePath);
    for (const name of fixture.dirs) await fs.mkdir(join(sourcePath, name));
    for (const name of fixture.files) await fs.writeFile(join(sourcePath, name), "");
    resources.set(fixture.name, await buildDirectoryResource("skill://oracle/listing", sourcePath));
  }
}

const matrix = [
  ["plain.txt", "", false], ["plain.txt", "", true],
  ["plain.txt", "3-4", false], ["plain.txt", "3-4", true],
  ["plain.txt", "raw", true], ["plain.txt", "raw:3-4", true],
  ["plain.txt", "raw:-2", true], ["plain.txt", "raw:1-1,8-9", true],
  ["plain.txt", "1-1,8-8,99-99", true], ["plain.txt", "99", true],
  ["plain.txt", "-2", true],
  ["empty.txt", "", true], ["empty.txt", "raw", true],
  ["crlf.txt", "raw:2-2", true], ["crlf.txt", "3-4", true],
  ["listing", "", true], ["listing", "3-3", true], ["listing", "raw:3-3", true],
  ["listing", "raw:-2", true], ["listing", "raw:1-1,8-8", true],
  ["blocks.ts", "1-1,4-4", true], ["blocks.ts", "raw:1-1,4-4", true],
];
const cases = [];
for (const [fixture, selector, lineNumbers] of matrix) {
  const resource = resources.get(fixture);
  const session = {
    cwd: fixtureRoot,
    hasEditTool: true,
    settings: {
      getEditVariantForModel: () => "hashline",
      get(key) {
        if (key === "readLineNumbers") return lineNumbers;
        throw new Error(`Unexpected session.settings.get(${key})`);
      },
    },
  };
  const result = buildInMemorySelectorResult(session, resource.content, parseSel(selector), {
    sourcePath: resource.sourcePath,
    entityLabel: "resource",
    immutable: true,
    ignoreResultLimits: true,
  });
  if (result.isError) throw new Error(`Renderer returned isError for ${fixture}:${selector}`);
  if (result.content.length !== 1 || result.content[0].type !== "text" || typeof result.content[0].text !== "string") {
    throw new Error(`Unexpected renderer content for ${fixture}:${selector}`);
  }
  if (!Number.isInteger(result.details?.totalLines) || result.details.totalLines < 0) {
    throw new Error(`Missing totalLines for ${fixture}:${selector}`);
  }
  cases.push({
    id: `${fixture}:${selector || "default"}:${lineNumbers ? "numbered" : "plain"}`,
    fixture,
    selector,
    lineNumbers,
    expected: { isError: false, text: result.content[0].text, totalLines: result.details.totalLines },
  });
}
if (cases[0].expected.text === cases[1].expected.text || cases[2].expected.text === cases[3].expected.text) {
  throw new Error("Line-number settings were not observed by the real display-mode resolver");
}
const totalCalls = Object.values(mockCounts).reduce((sum, count) => sum + count, 0);
if (totalCalls !== 0) throw new Error("A dead dependency was invoked");
if (nativeCalls < 1) throw new Error("No real native block-context calls were observed");
await verifySourceBytes();
await fs.writeFile(join(runDir, "native-call-receipts.json"), JSON.stringify(nativeCallReceipts, null, 2) + "\n");
console.log(JSON.stringify({
  schemaVersion: 1,
  upstreamCommit: UPSTREAM_COMMIT,
  bunVersion: Bun.version,
  platform: process.platform,
  native: manifest.native,
  sourceManifest: manifest.sourceManifest,
  mocks: { exports: mockCounts, totalCalls },
  nativeCalls,
  fixtures,
  cases,
}, null, 2));
