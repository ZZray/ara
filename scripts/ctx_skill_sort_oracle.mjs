// Run with Bun 1.4.0 and a separately fetched copy of OMP's fixed
// packages/coding-agent/src/internal-urls/filesystem-resource.ts.
import { createHash } from "node:crypto";
import * as fs from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const SOURCE_SHA256 = "a20c01f819dac9d33f14ba588a76d814193f7ec0d64aee60089d27e5a5322625";
const sourcePath = resolve(process.argv[2] ?? "");
if (!process.argv[2]) throw new Error("Pass the fixed-OMP filesystem-resource.ts path");
if (globalThis.Bun?.version !== "1.4.0") throw new Error("Run this oracle with Bun 1.4.0");
const source = await fs.readFile(sourcePath);
const sourceSha256 = createHash("sha256").update(source).digest("hex");
if (sourceSha256 !== SOURCE_SHA256) throw new Error(`Fixed OMP source hash mismatch: ${sourceSha256}`);
const { buildDirectoryResource } = await import(pathToFileURL(sourcePath).href);

const cases = [
  { name: "portable", dirs: ["z-dir", "A-dir", "é-dir"], files: [
    "a_1.txt", "a-1.txt", "A2.txt", "a10.txt", "b.txt", "B2.txt",
    "é.txt", "e\u0301.txt", "ä.txt", "中.txt", "Ω.txt", "📄.txt",
    ".hidden", "1.txt",
  ] },
];
if (process.platform !== "win32") {
  cases.push({ name: "case-collision", dirs: [], files: ["a.txt", "A.txt", "ä.txt", "Ä.txt"] });
}

const output = {
  upstreamCommit: "596f2da7101178214aa27a753529d15e6b7ad91d",
  sourceSha256,
  bunVersion: globalThis.Bun?.version ?? null,
  platform: process.platform,
  arch: process.arch,
  nodeIcuVersion: process.versions?.icu ?? null,
  localeEnv: { LANG: process.env.LANG ?? null, LC_ALL: process.env.LC_ALL ?? null, LC_COLLATE: process.env.LC_COLLATE ?? null },
  collator: new Intl.Collator().resolvedOptions(),
  cases: [],
};

for (const fixture of cases) {
  const root = await fs.mkdtemp(join(tmpdir(), "ara-ctx-sort-"));
  try {
    for (const name of fixture.dirs) await fs.mkdir(join(root, name));
    for (const name of fixture.files) await fs.writeFile(join(root, name), name);
    const sampledRawBefore = (await fs.readdir(root, { withFileTypes: true })).map(entry => ({
      name: entry.name,
      isDirectory: entry.isDirectory(),
    }));
    if (sampledRawBefore.length !== fixture.dirs.length + fixture.files.length) {
      throw new Error(`${fixture.name}: filesystem collapsed distinct names`);
    }
    const resource = await buildDirectoryResource("skill://oracle", root);
    const sampledRawAfter = (await fs.readdir(root, { withFileTypes: true })).map(entry => ({
      name: entry.name,
      isDirectory: entry.isDirectory(),
    }));
    if (JSON.stringify(sampledRawBefore) !== JSON.stringify(sampledRawAfter)) {
      throw new Error(`${fixture.name}: enumeration order changed during oracle`);
    }
    const lines = resource.content.split("\n");
    output.cases.push({
      name: fixture.name,
      sampledRawBefore,
      sampledRawAfter,
      content: resource.content,
      contentSha256: createHash("sha256").update(resource.content, "utf8").digest("hex"),
      projectionOnly: true,
      projectedRows: Object.fromEntries(lines.map((line, index) => [String(index + 1), line])),
    });
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
}

console.log(JSON.stringify(output, null, 2));
