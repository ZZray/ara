// The Python launcher exports the exact fixed Git blobs and their license.
// This runner imports unchanged Skill and prompt modules; dead imports trap.
import { mock } from "bun:test";
import { createHash } from "node:crypto";
import * as fs from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const UPSTREAM_COMMIT = "596f2da7101178214aa27a753529d15e6b7ad91d";
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
for (const source of [...manifest.sourceManifest, manifest.license]) {
  const bytes = await fs.readFile(join(runDir, "upstream", source.path));
  if (digest(bytes) !== source.sha256) throw new Error(`Executed source hash mismatch: ${source.path}`);
  const gitBlob = createHash("sha1").update(`blob ${bytes.length}\0`).update(bytes).digest("hex");
  if (gitBlob !== source.gitBlob) throw new Error(`Executed Git blob mismatch: ${source.path}`);
}

const codingRoot = join(runDir, "upstream", "packages", "coding-agent", "src");
const utilsRoot = join(runDir, "upstream", "packages", "utils", "src");
let mockCalls = 0;
function trap(name) {
  return (..._args) => {
    mockCalls++;
    throw new Error(`Unrelated Skill dependency invoked: ${name}`);
  };
}
async function mockLocal(relativePath, exports) {
  // Bun resolves a relative import before applying its mock. These sentinels
  // are not upstream exports and must never execute in this oracle.
  const target = join(codingRoot, relativePath);
  await fs.mkdir(dirname(target), { recursive: true });
  await fs.writeFile(target, `throw new Error(${JSON.stringify(`Dead dependency loaded: ${relativePath}`)});\n`);
  mock.module(target, () => exports);
}
await mockLocal("autolearn/managed-skills.ts", {
  isValidManagedSkillName: trap("isValidManagedSkillName"),
  MANAGED_SKILLS_PROVIDER_ID: "oracle-unused-managed-provider",
  sanitizeManagedDescription: trap("sanitizeManagedDescription"),
});
await mockLocal("capability/skill.ts", { skillCapability: trap("skillCapability") });
await mockLocal("discovery/index.ts", {
  isUserSourceEnabled: trap("isUserSourceEnabled"),
  loadCapability: trap("loadCapability"),
});
await mockLocal("discovery/helpers.ts", {
  compareSkillOrder: trap("compareSkillOrder"),
  scanSkillsFromDir: trap("scanSkillsFromDir"),
});
await mockLocal("tools/path-utils.ts", { expandTilde: trap("expandTilde") });

// Use the exact pinned prompt renderer and template engine. Only the pi-utils
// package facade is mocked so skills.ts can resolve its unrelated export.
const prompt = await import(pathToFileURL(join(utilsRoot, "prompt.ts")).href);
mock.module("@oh-my-pi/pi-utils", () => ({
  prompt,
  getProjectDir: trap("getProjectDir"),
}));
const { parseSkillInvocation, buildSkillPromptMessage } = await import(
  pathToFileURL(join(codingRoot, "extensibility", "skills.ts")).href
);

const cases = [];
const parseInputs = [
  ["leading-bare", "/skill:foo"],
  ["leading-args", "/skill:foo focus on auth"],
  ["leading-whitespace", "  /skill:foo focus on auth"],
  ["leading-empty-name", "/skill:"],
  ["mid-trailing", "fix the auth bug /skill:security-scan "],
  ["mid-both-sides", "leading /skill:foo trailing"],
  ["mid-newline-prose", "explain this\nthen use /skill:security-scan "],
  ["other-slash-command", "/compact /skill:security-scan"],
  ["other-slash-goal", "/goal set /skill:foo focus on auth"],
  ["bash-prefix", "!echo /skill:reviewer"],
  ["bash-double-prefix", "!!echo /skill:reviewer"],
  ["python-prefix", "$ run.py /skill:foo"],
  ["python-double-prefix", "$$ run.py /skill:foo"],
  ["python-tab-prefix", "$\trun /skill:foo"],
  ["dollar-prose", "$echo /skill:reviewer"],
  ["dollar-braces", "${HOME}/bin /skill:foo"],
  ["no-token", "no skill token here"],
  ["url-glued", "https://example.com/skill:foo"],
  ["mid-slash-name", "see /skill:foo/bar"],
  ["leading-tab", "\t/skill:foo bar"],
  ["leading-tab-in-name", "/skill:foo\tbar"],
  ["leading-slash-name", "/skill:foo/bar"],
  ["mid-nbsp", "before\u00a0/skill:foo\u00a0after"],
  ["mid-zero-width", "before\u200b/skill:foo after"],
  ["first-of-two", "before /skill:foo middle /skill:bar after"],
];
for (const [id, input] of parseInputs) {
  cases.push({ kind: "parse", id, input, result: parseSkillInvocation(input) ?? null });
}

function utf8(text) { return Buffer.from(text, "utf8"); }
const builders = [
  { id: "lf-frontmatter", bytes: utf8("---\nname: reviewer\ndescription: Review code\n---\n\nReview the supplied code carefully.\n"), args: " focus on risks " },
  { id: "no-args", bytes: utf8("Do the thing.\n"), args: " \t" },
  { id: "crlf-frontmatter", bytes: utf8("---\r\nname: reviewer\r\n---\r\n\r\nBody.\r\n"), args: "check CRLF" },
  { id: "bom-frontmatter", bytes: Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), utf8("---\nname: reviewer\n---\nBody.\n")]), args: "BOM" },
  { id: "invalid-utf8", bytes: Buffer.from([0x42, 0x6f, 0x64, 0x79, 0x3a, 0x20, 0xff, 0x0a]), args: "decoder" },
  { id: "html-comment", bytes: utf8("<!-- note -->\nUse <tag> & review.\n"), args: " <unsafe> & " },
  { id: "empty-body", bytes: utf8("---\nname: reviewer\n---\n\n \t\n"), args: "" },
  { id: "multiline-body", bytes: utf8("Line one.\n\nLine three.\n"), args: "\u00a0line\u00a0" },
  { id: "fresh-reread", priorBytes: utf8("Old instructions.\n"), bytes: utf8("New instructions.\n"), args: "current" },
  { id: "missing-file", bytes: null, args: "no file" },
];
for (const fixture of builders) {
  const baseDir = join(runDir, "fixtures", fixture.id);
  const filePath = join(baseDir, "SKILL.md");
  await fs.mkdir(baseDir, { recursive: true });
  const skill = { name: "reviewer", filePath, baseDir };
  const item = {
    kind: "build", id: fixture.id, skill, args: fixture.args,
    contentBytesBase64: fixture.bytes?.toString("base64") ?? null,
    priorContentBytesBase64: fixture.priorBytes?.toString("base64") ?? null,
  };
  if (fixture.priorBytes) {
    await fs.writeFile(filePath, fixture.priorBytes);
    item.priorResult = { ok: true, ...await buildSkillPromptMessage(skill, fixture.args, "user") };
  }
  if (fixture.bytes) await fs.writeFile(filePath, fixture.bytes);
  try {
    item.result = { ok: true, ...await buildSkillPromptMessage(skill, fixture.args, "user") };
  } catch (error) {
    item.result = { ok: false, name: error?.name ?? null, message: String(error?.message ?? error) };
  }
  if (fixture.id === "fresh-reread") {
    const before = item.priorResult?.message ?? "";
    const after = item.result?.message ?? "";
    if (!before.includes("Old instructions.") || before.includes("New instructions.") ||
        !after.includes("New instructions.") || after.includes("Old instructions.")) {
      throw new Error("Fixed Skill builder did not reread changed file contents");
    }
  }
  cases.push(item);
}

if (mockCalls !== 0) throw new Error(`Dead dependency called ${mockCalls} times`);
const oracle = {
  upstreamCommit: UPSTREAM_COMMIT,
  bunVersion: Bun.version,
  sourceManifest: manifest.sourceManifest,
  license: manifest.license,
  runnerSha256: manifest.runnerSha256,
  cases,
  mockCalls,
};
await fs.writeFile(join(runDir, "oracle.json"), JSON.stringify(oracle, null, 2) + "\n");
process.stdout.write(JSON.stringify({ cases: cases.length, mockCalls }) + "\n");
