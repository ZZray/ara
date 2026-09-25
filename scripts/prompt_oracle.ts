#!/usr/bin/env bun
/**
 * Differential oracle for crates/ara-prompt: runs OMP's own
 * packages/utils/src/{template,prompt}.ts (pinned checkout) on generated
 * cases and on OMP's prompt corpus, and writes the upstream results as JSON
 * for the Rust tests to compare against.
 *
 *   bun scripts/prompt_oracle.ts cases  <omp-checkout> <out.json>
 *   bun scripts/prompt_oracle.ts corpus <omp-checkout> <out.json>
 *
 * `cases` output is committed (crates/ara-prompt/tests/fixtures/oracle.json);
 * `corpus` output is large and consumed via ARA_PROMPT_CORPUS_ORACLE.
 */
import * as fs from "node:fs";
import * as path from "node:path";

const [mode, ompRoot, outPath] = process.argv.slice(2);
if (!mode || !ompRoot || !outPath) {
	console.error("usage: prompt_oracle.ts cases|corpus <omp-checkout> <out.json>");
	process.exit(2);
}
const utils = path.resolve(ompRoot, "packages/utils/src");
const prompt = await import(path.join(utils, "prompt.ts"));
const template = await import(path.join(utils, "template.ts"));

const PROMPT_SOURCE = { renderPhase: "pre-render", replaceAsciiSymbols: true, normalizeRfc2119: true } as const;

type Outcome = { ok: string } | { error: string };
function run(fn: () => string): Outcome {
	try {
		return { ok: fn() };
	} catch (error) {
		return { error: error instanceof Error ? error.message : String(error) };
	}
}

// The escaping engine carries the helpers from upstream template.test.ts.
function testEngine() {
	const engine = template.create();
	engine.registerHelper("eq", (left: unknown, right: unknown) => left === right);
	engine.registerHelper("label", (value: unknown, options: { hash: Record<string, unknown> }) => `${options.hash.prefix}:${value}`);
	engine.registerHelper("choose", function (this: unknown, value: unknown, options: any) {
		return value === options.hash.expected ? options.fn(this) : options.inverse(this);
	});
	engine.registerHelper("safe", (value: unknown) => new template.SafeString(value));
	return engine;
}

// mulberry32: a small seeded PRNG so the generated corpus is reproducible.
function rng(seed: number) {
	let a = seed >>> 0;
	const next = () => {
		a = (a + 0x6d2b79f5) >>> 0;
		let t = a;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
	return { next, pick: <T>(items: readonly T[]): T => items[Math.floor(next() * items.length)] };
}

const CONTEXTS = [
	{
		name: "Ada",
		zero: 0,
		empty: [],
		flag: true,
		off: false,
		n: 2,
		s: "10",
		items: [1, 2, "three", null, false],
		user: { name: "Bob", "display name": "B", age: 3, tags: ["x", "y"] },
		html: '<a href="x">&\'`=</a>',
		rows: [
			{ a: 1, b: "x" },
			{ a: 2, b: " y " },
		],
		args: ["one", "two"],
		groups: [{ name: "G", items: [0, "", "z"] }],
	},
	{
		name: "",
		zero: 0,
		empty: [],
		flag: false,
		off: true,
		n: 1,
		s: "1",
		items: [],
		user: { name: "", age: 0 },
		html: "",
		rows: [],
		args: "xyz",
		groups: [],
	},
];

const LEAVES = [
	"a",
	" ",
	"\n",
	"  ",
	"x\n",
	"<tag>",
	"</tag>",
	"| a | b |",
	"\n\n\n",
	" -> ",
	"&<>\"'`=",
	"é",
	"\t",
	"{",
	"}",
	"{{name}}",
	"{{{html}}}",
	"{{&html}}",
	"{{html}}",
	"{{user.name}}",
	"{{user.[display name]}}",
	"{{user.tags.[1]}}",
	"{{items.length}}",
	"{{items.[2]}}",
	"{{items.[02]}}",
	"{{this}}",
	"{{this.name}}",
	"{{.}}",
	"{{@index}}",
	"{{@key}}",
	"{{@first}}",
	"{{@last}}",
	"{{../name}}",
	"{{@root.name}}",
	"{{@root}}",
	"{{missing.x}}",
	"{{lookup items 1}}",
	'{{lookup user "name"}}',
	"{{lookup user missing}}",
	"{{! c }}",
	"{{!-- c --}}",
	"{{len items}}",
	"{{len name}}",
	"{{len user}}",
	"{{add n 2}}",
	'{{add n "1"}}',
	"{{add name 1}}",
	"{{add missing missing}}",
	"{{sub n 5}}",
	'{{sub s "3"}}',
	'{{default missing "fb"}}',
	"{{default name zero}}",
	'{{pluralize n "item" "items"}}',
	'{{pluralize s "item" "items"}}',
	'{{join items ", "}}',
	'{{join items "\\n"}}',
	"{{join items}}",
	"{{join name}}",
	"{{escapeXml html}}",
	"{{escapeXml missing}}",
	"{{jsonStringify user}}",
	"{{jsonStringify items}}",
	"{{jsonStringify name}}",
	"{{not zero}}",
	"{{not items}}",
	"{{includes items 2}}",
	'{{includes items "2"}}',
	"{{includes name 2}}",
	"{{arg 1}}",
	'{{arg "2"}}',
	"{{arg 0}}",
	'{{arg " 3px"}}',
	"{{arg 1.5}}",
	"{{n}}",
	"{{zero}}",
	"{{off}}",
	"{{empty}}",
	"{{items}}",
	"{{user}}",
] as const;

// Helpers registered only on the escaping test engine, and deliberate misses.
const ENGINE_LEAVES = ["{{eq n 2}}", '{{label (eq n 2) prefix="p"}}', "{{safe html}}"] as const;
const ERROR_LEAVES = ["{{missingHelper n}}", '{{missingHelper k="v"}}', "{{eq n 2}}", "{{#if flag}}x{{/each}}", "{{#if flag}}x", "{{!-- open", "{{name"] as const;
let engineMode = false;
// The escaping engine has only the built-ins, not the prompt helpers.
const PROMPT_HELPER = /\{\{\(?(?:len|add|sub|default|pluralize|join|escapeXml|jsonStringify|not|includes|arg)\b|\(not /;
const CORE_LEAVES = LEAVES.filter(leaf => !PROMPT_HELPER.test(leaf));

const CONDITIONS = ["name", "zero", "empty", "items", "missing", "flag", "off", "user", "(not flag)", "n", "s"] as const;
const WHEN = ['n ">" 1', 'n "<" 1', 'n ">=" 2', 'n "<=" 1', 's ">" "9"', 's "<" 9', 'n "==" 2', 'n "===" "2"', 'n "!=" 2', 'n "!==" "2"', 'n "~" 2', "name \"==\" \"Ada\""] as const;

function block(r: ReturnType<typeof rng>, depth: number): string {
	const body = () => fragment(r, depth + 1, 3);
	const cond = r.pick(CONDITIONS);
	const standalone = r.next() < 0.3;
	const wrap = (open: string, inner: string, close: string, inverse?: string) => {
		const elseTag = inverse === undefined ? "" : standalone ? `\n  {{else}}\n${inverse}` : `{{else}}${inverse}`;
		return standalone ? `\n  ${open}  \n${inner}${elseTag}\n  ${close}\n` : `${open}${inner}${elseTag}${close}`;
	};
	const maybeInverse = () => (r.next() < 0.5 ? body() : undefined);
	switch (Math.floor(r.next() * 17)) {
		case 0:
			return wrap(`{{#if ${cond}}}`, body(), "{{/if}}", maybeInverse());
		case 1:
			return wrap(`{{#unless ${cond}}}`, body(), "{{/unless}}", maybeInverse());
		case 2:
			return wrap(`{{#each ${r.pick(["items", "user", "empty", "name", "groups", "rows", "missing"])}}}`, body(), "{{/each}}", maybeInverse());
		case 3:
			return wrap(`{{#with ${r.pick(["user", "missing", "empty", "zero", "items"])}}}`, body(), "{{/with}}", maybeInverse());
		case 4:
			return wrap(`{{#${r.pick(["user", "items", "name", "flag", "off", "zero", "missing"])}}}`, body(), `{{/${"__NAME__"}}}`, maybeInverse());
		case 5:
			return wrap(`{{^${cond.startsWith("(") ? "flag" : cond}}}`, body(), `{{/${cond.startsWith("(") ? "flag" : cond}}}`, maybeInverse());
		case 6:
			return wrap(`{{#list ${r.pick(["items", "rows", "empty", "name"])} prefix="- " ${r.pick(['join="\\n"', 'join=", "', "", 'suffix=";"'])}}}`, body(), "{{/list}}");
		case 7:
			return wrap(`{{#when ${r.pick(WHEN)}}}`, body(), "{{/when}}", maybeInverse());
		case 8:
			return wrap(`{{#ifAny ${cond} zero}}`, body(), "{{/ifAny}}", maybeInverse());
		case 9:
			return wrap(`{{#ifAll ${cond} flag}}`, body(), "{{/ifAll}}", maybeInverse());
		case 10:
			return wrap(`{{#has ${r.pick(['user "name"', 'user "toString"', "items 2", 'items "2"', "user 3", "name 1"])}}}`, body(), "{{/has}}", maybeInverse());
		case 11:
			return wrap(`{{#table rows${r.pick([' headers="A|B"', "", ' headers=""'])}}}`, "{{a}} | {{b}}", "{{/table}}");
		case 12:
			return wrap(`{{#codeblock${r.pick([' lang="ts"', ""])}}}`, body(), "{{/codeblock}}");
		case 13:
			return wrap(`{{#xml "${r.pick(["t", "files"])}"}}`, body(), "{{/xml}}");
		case 14:
			return wrap(`{{#choose n expected=2}}`, body(), "{{/choose}}", maybeInverse());
		case 15:
			return wrap("{{#each groups}}", "{{@index}}:{{#each items}}{{../name}}/{{@root.name}}/{{@index}}/{{@last}}={{this}};{{/each}}", "{{/each}}");
		default:
			return wrap("{{#if flag}}", body(), "{{/if}}", maybeInverse());
	}
}

function fragment(r: ReturnType<typeof rng>, depth: number, max: number): string {
	let out = "";
	const count = 1 + Math.floor(r.next() * max);
	for (let i = 0; i < count; i++) {
		if (depth < 3 && r.next() < 0.35) out += block(r, depth);
		else if (engineMode && r.next() < 0.1) out += r.pick(ENGINE_LEAVES);
		else out += r.pick(engineMode ? CORE_LEAVES : LEAVES);
	}
	return out;
}

function fixBlockNames(source: string): string {
	// `{{#name}}…{{/__NAME__}}` sections close with their own name.
	const stack: string[] = [];
	return source.replace(/\{\{([#^/])([^\s}]+)[^}]*\}\}/g, (match, sigil: string, name: string) => {
		if (sigil === "/") {
			const open = stack.pop() ?? name;
			return name === "__NAME__" ? `{{/${open}}}` : match;
		}
		stack.push(name);
		return match;
	});
}

const FORMAT_LINES = [
	"",
	" ",
	"\t",
	"  text",
	"text  ",
	"<tag>",
	"</tag>",
	"  </tag>",
	'<tag attr="x">',
	"<a b> c>",
	"<self/>",
	"<Tag>",
	"</a-b_c>",
	"<x y>",
	"| a | b |",
	"|:--- | --:|",
	"| :-: | - |",
	"  | c |",
	"|||",
	"```",
	"~~~ts",
	"  ```",
	"{{#if x}}",
	"{{/if}}",
	"  {{/if}}",
	"<!-- a -> b",
	"-->",
	"<!-- x --> y -> z <!-- w",
	"a -> b <= c ... d != e >= f <-> g <- h",
	"....... ..",
	"**MUST** do",
	"MUST NOT x `MUST NOT` y",
	"**SHOULD NOT** and SHOULD NOT and XMUST NOT",
	"**MAY**/**NEVER**/**AVOID**/**bold**",
	"  nbsp",
	"　ideographic",
	"é -> ü",
	"- item",
	"x --> y",
	" sep",
	"a\rb",
] as const;

function generateCases(): unknown[] {
	const r = rng(0x5eed);
	const cases: unknown[] = [];
	for (let i = 0; i < 400; i++) {
		const op = r.next() < 0.5 ? "render" : "engine";
		engineMode = op === "engine";
		let source = fixBlockNames(fragment(r, 0, 5));
		if (i % 40 === 39) source += r.pick(ERROR_LEAVES);
		const contextIndex = Math.floor(r.next() * CONTEXTS.length);
		const context = CONTEXTS[contextIndex];
		const result =
			op === "render"
				? run(() => prompt.render(source, context))
				: run(() => testEngine().compile(source)(context));
		cases.push({ id: `t${i}`, op, source, context: contextIndex, ...result });
	}
	for (let i = 0; i < 300; i++) {
		const lineCount = 1 + Math.floor(r.next() * 12);
		const source = Array.from({ length: lineCount }, () => r.pick(FORMAT_LINES)).join("\n");
		const options = {
			renderPhase: r.next() < 0.5 ? "pre-render" : "post-render",
			replaceAsciiSymbols: r.next() < 0.5,
			normalizeRfc2119: r.next() < 0.5,
		};
		cases.push({ id: `f${i}`, op: "format", source, options, ...run(() => prompt.format(source, options)) });
	}
	return cases;
}

if (mode === "cases") {
	const cases = generateCases();
	const body = `{\n"contexts": ${JSON.stringify(CONTEXTS)},\n"cases": [\n${cases.map(c => JSON.stringify(c)).join(",\n")}\n]\n}\n`;
	fs.writeFileSync(outPath, body);
	const errors = cases.filter(c => "error" in (c as object)).length;
	console.log(`wrote ${cases.length} cases (${errors} upstream errors) to ${outPath}`);
} else if (mode === "corpus") {
	const glob = new Bun.Glob("packages/*/src/**/*.md");
	const files: unknown[] = [];
	for await (const relative of glob.scan(ompRoot)) {
		const source = fs.readFileSync(path.join(ompRoot, relative), "utf8");
		files.push({
			path: relative,
			compile: run(() => (prompt.compile(source), "ok")),
			source_format: run(() => prompt.format(source, PROMPT_SOURCE)),
			post_format: run(() => prompt.format(source)),
		});
	}
	files.sort((a: any, b: any) => (a.path < b.path ? -1 : 1));
	fs.writeFileSync(outPath, JSON.stringify({ files }));
	console.log(`wrote ${files.length} corpus files to ${outPath}`);
} else {
	console.error(`unknown mode ${mode}`);
	process.exit(2);
}
