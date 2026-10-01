// Fixed OMP schema IR inventory/generation. Rust never executes this script.
import { createHash } from "node:crypto";
const input = JSON.parse(await Bun.stdin.text());
const transpiler = new Bun.Transpiler({ loader: "ts" });
const output = [];
for (const item of input) {
  const names = [...item.source.matchAll(/^export (?:const|enum) (\w+)/gm)].map(m => m[1]);
  const source = item.source.replace(/^import .*from "\.\/protobuf";\s*$/m, "").replace(/^export /gm, "");
  const pb = (typeName, fields = []) => ({ typeName, fields });
  const module = new Function("pb", `${transpiler.transformSync(source)};return {${names.join(",")}};`)(pb);
  const schemas = [], enums = [];
  for (const [name, value] of Object.entries(module)) {
    if (value.typeName !== undefined) {
      const normalize = field => ({ ...field, ...(field.T ? { T: field.T().typeName } : {}), ...(typeof field.V === "function" ? { V: field.V().typeName, messageValue: true } : {}), ...(field.variants ? { variants: field.variants.map(normalize) } : {}) });
      schemas.push({ export: name, typeName: value.typeName, fields: value.fields.map(normalize) });
    } else {
      enums.push({ export: name, values: Object.entries(value).filter(([k]) => Number.isNaN(Number(k))) });
    }
  }
  const typeNames = new Set(schemas.map(s => s.typeName));
  for (const schema of schemas) for (const f of schema.fields) for (const v of [f, ...(f.variants ?? [])]) {
    if (v.T && !typeNames.has(v.T)) throw new Error(`Unresolved schema ${v.T}`);
    if (v.messageValue && !typeNames.has(v.V)) throw new Error(`Unresolved map ${v.V}`);
  }
  output.push({ module: item.module, sourcePath: item.path, sourceSha256: createHash("sha256").update(item.source).digest("hex"), schemas, enums });
}
console.log(JSON.stringify(output));
