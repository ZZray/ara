"""Generate every fixed OMP Cursor/Devin schema, without narrowing by consumers."""
import argparse
import hashlib
import json
import pathlib
import subprocess

SHA = "596f2da7101178214aa27a753529d15e6b7ad91d"
PINS = {
    "cursor": "733e4526bf073c388f6b237038dbde7f8d43e669fceb527360f68d561af2e01b",
    "devin": "a06e2dc3bac69ea93b50d9ff4b230102fec5f1cb1ac6ef05b6cb04ee212c3034",
}
LICENSE = """// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the \"Software\"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
"""

def rust_string(value):
    return json.dumps(value, ensure_ascii=False)

def main():
    args = argparse.ArgumentParser()
    args.add_argument("--upstream", required=True)
    args.add_argument("--bun", required=True)
    args.add_argument("--output", default="crates/ara-cli/src/catalog_proto_schemas")
    opt = args.parse_args()
    sources = []
    for module, pin in PINS.items():
        path = f"packages/catalog/src/discovery/{module}-proto.ts"
        data = subprocess.check_output(["git", "-C", opt.upstream, "show", SHA + ":" + path])
        if hashlib.sha256(data).hexdigest() != pin:
            raise ValueError("wrong fixed schema source")
        sources.append({"module": module, "path": path, "source": data.decode()})
    runner = pathlib.Path(__file__).with_suffix(".mjs")
    result = subprocess.run([opt.bun, str(runner)], input=json.dumps(sources), encoding="utf-8", text=True, capture_output=True, check=True)
    ir = json.loads(result.stdout)
    output = pathlib.Path(opt.output)
    output.mkdir(parents=True, exist_ok=True)
    for module in ir:
        expected = (571, 12) if module["module"] == "cursor" else (64, 34)
        if (len(module["schemas"]), len(module["enums"])) != expected:
            raise ValueError("incomplete fixed export inventory")
        text = ["// Generated from fixed OMP " + SHA + "; do not edit.\n" + LICENSE,
                "#![allow(non_upper_case_globals, non_camel_case_types)]\n",
                "use crate::catalog_protobuf::{SchemaHandle, ProtoMessage};\n"]
        for schema in module["schemas"]:
            name = schema["export"]
            shape = name.removesuffix("Schema")
            text.append(f"pub type {shape} = ProtoMessage;\npub const {name}: SchemaHandle = SchemaHandle::new({rust_string(module['module'])}, {rust_string(schema['typeName'])});\n")
        for enum in module["enums"]:
            # Open int32: future values remain representable, including Devin 6/7/8.
            text.append(f"pub mod {enum['export']} {{\n")
            for name, number in enum["values"]:
                text.append(f"    pub const {name}: i32 = {number};\n")
            text.append("}\n")
        (output / (module["module"] + ".rs")).write_text("".join(text), encoding="utf-8", newline="\n")
    (output / "ir.json").write_text(json.dumps({"upstreamCommit": SHA, "modules": ir}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8", newline="\n")
    (output / "mod.rs").write_text("//! Complete fixed Cursor/Devin public schema facade.\n// Fixed upstream enum namespaces retain their public PascalCase export names.\n#![allow(non_snake_case)]\npub mod cursor;\npub mod devin;\npub const IR: &str = include_str!(\"ir.json\");\n", encoding="utf-8", newline="\n")
    print(json.dumps({"schemas": sum(len(m["schemas"]) for m in ir), "enums": sum(len(m["enums"]) for m in ir), "irSha256": hashlib.sha256((output / "ir.json").read_bytes()).hexdigest()}))

if __name__ == "__main__":
    main()
