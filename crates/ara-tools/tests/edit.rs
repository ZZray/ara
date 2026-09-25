//! `edit` against real files through the vendored pi-edit engine, and the
//! hashline anchors `read`/`grep` hand to it (OMP `edit/index.ts`,
//! `read.ts`/`grep.ts` hashline display).

use ara_agent::{AgentTool, Concurrency, ToolOutput};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::edit::EditTool;
use ara_tools::grep::GrepTool;
use ara_tools::read::ReadTool;
use ara_tools::write::WriteTool;
use ara_tools::{ToolContext, builtin_tools};
use pi_edit::EditMode;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn args(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

fn text(o: &ToolOutput) -> String {
    o.content
        .iter()
        .filter_map(|b| if let UserBlock::Text(t) = b { Some(t.text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n<<>>\n")
}

async fn run(t: &dyn AgentTool, v: Value) -> ToolOutput {
    t.execute("c", args(v), CancellationToken::new(), Arc::new(|_| {})).await.unwrap()
}

fn tag(root: &Path, rel: &str) -> String {
    pi_edit::store::file_hash(&std::fs::read_to_string(root.join(rel)).unwrap())
}

const GREET: &str = "def greet(name):\n    msg = \"Hello, \" + name\n    print(msg)\ngreet(\"world\")\n";

struct Tools {
    read: ReadTool,
    grep: GrepTool,
    edit: EditTool,
}

fn tools(root: &Path, mode: EditMode) -> Tools {
    let ctx = ToolContext::new(root).with_edit(mode, true);
    Tools { read: ReadTool { ctx: ctx.clone() }, grep: GrepTool::new(ctx.clone()), edit: EditTool::new(ctx) }
}

#[tokio::test]
async fn replace_mode_results_and_errors() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("a.py"), GREET).unwrap();
    let t = tools(r, EditMode::Replace);
    // Replace mode keeps plain reads (no hashline anchors).
    assert_eq!(text(&run(&t.read, json!({"path": "a.py"})).await), GREET.trim_end());
    let out =
        run(&t.edit, json!({"path": "a.py", "old_string": "print(msg)", "new_string": "print(msg.upper())"})).await;
    assert!(!out.is_error);
    assert_eq!(
        text(&out),
        "[a.py]\n1:def greet(name):\n2:    msg = \"Hello, \" + name\n3:    print(msg.upper())\n4:greet(\"world\")"
    );
    let details = out.details.unwrap();
    assert_eq!(details["op"], "update");
    assert_eq!(details["firstChangedLine"], 3);
    assert!(details["diff"].as_str().unwrap().contains("-3|    print(msg)"), "{details}");
    assert_eq!(std::fs::read_to_string(r.join("a.py")).unwrap(), GREET.replace("print(msg)", "print(msg.upper())"));

    let missing = run(&t.edit, json!({"path": "a.py", "old_string": "nope nope", "new_string": "x"})).await;
    assert!(missing.is_error);
    assert!(text(&missing).starts_with("Could not find a close enough match in a.py."), "{}", text(&missing));
    let multi = run(&t.edit, json!({"path": "a.py", "old_string": "msg", "new_string": "m"})).await;
    assert!(multi.is_error && text(&multi).starts_with("Found 2 occurrences in a.py:"));
    assert!(text(&multi).ends_with("Add more context lines to disambiguate."));
    let all = run(&t.edit, json!({"path": "a.py", "old_string": "msg", "new_string": "m", "replace_all": true})).await;
    assert!(!all.is_error, "{}", text(&all));
    assert!(!std::fs::read_to_string(r.join("a.py")).unwrap().contains("msg"));
    let nofile = run(&t.edit, json!({"path": "zz.py", "old_string": "a", "new_string": "b"})).await;
    assert!(nofile.is_error);
    assert_eq!(text(&nofile), "File not found: zz.py");
    // Fuzzy whitespace match (edit.fuzzyMatch default true).
    std::fs::write(r.join("w.txt"), "alpha  beta\n").unwrap();
    let fuzzy = run(&t.edit, json!({"path": "w.txt", "old_string": "alpha beta", "new_string": "gamma"})).await;
    assert!(!fuzzy.is_error, "{}", text(&fuzzy));
    assert_eq!(std::fs::read_to_string(r.join("w.txt")).unwrap(), "gamma\n");
}

#[tokio::test]
async fn hashline_anchors_flow_from_read_and_grep_to_edit() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("a.py"), GREET).unwrap();
    let t = tools(r, EditMode::Hashline);
    let first = tag(r, "a.py");
    assert_eq!(
        text(&run(&t.read, json!({"path": "a.py"})).await),
        format!(
            "[a.py#{first}]\n1:def greet(name):\n2:    msg = \"Hello, \" + name\n3:    print(msg)\n4:greet(\"world\")"
        )
    );
    assert_eq!(
        text(&run(&t.grep, json!({"pattern": "msg", "path": "a.py"})).await),
        format!(
            "[a.py#{first}]\n 1:def greet(name):\n*2:    msg = \"Hello, \" + name\n*3:    print(msg)\n 4:greet(\"world\")"
        )
    );
    let put = run(&t.edit, json!({"input": format!("[a.py#{first}]\nPUT 3.=3:\n+    print(msg + \"!\")\n")})).await;
    let second = tag(r, "a.py");
    assert_ne!(first, second);
    assert_eq!(
        text(&put),
        format!(
            "[a.py#{second}]\n1:def greet(name):\n2:    msg = \"Hello, \" + name\n3:    print(msg + \"!\")\n4:greet(\"world\")"
        )
    );
    // A stale tag is rejected with the current content and the new tag.
    let stale = run(&t.edit, json!({"input": format!("[a.py#{first}]\nPUT 3.=3:\n+    print(1)\n")})).await;
    assert!(stale.is_error);
    assert!(
        text(&stale).starts_with(&format!(
            "Edit rejected for a.py: file changed between read and edit.\nSection is bound to #{first}, but the current file hashes to #{second}."
        )),
        "{}",
        text(&stale)
    );
    assert!(std::fs::read_to_string(r.join("a.py")).unwrap().contains("print(msg + \"!\")"), "nothing written");

    // Grouped grep headers carry tags too.
    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::write(r.join("src/b.py"), "msg = 1\n").unwrap();
    let t_b = tag(r, "src/b.py");
    let grouped = text(&run(&t.grep, json!({"pattern": "msg = 1"})).await);
    assert_eq!(grouped, format!("# src/\n## b.py#{t_b}\n*1:msg = 1"));
    // Insert after the last line using the grep tag.
    let ins = run(&t.edit, json!({"input": format!("[src/b.py#{t_b}]\nPUT >1:\n+msg = 2\n")})).await;
    assert!(!ins.is_error, "{}", text(&ins));
    assert_eq!(std::fs::read_to_string(r.join("src/b.py")).unwrap(), "msg = 1\nmsg = 2\n");
}

#[tokio::test]
async fn hashline_blocks_moves_deletes_and_parse_warnings() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("lib.rs"), "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n").unwrap();
    let t = tools(r, EditMode::Hashline);
    // `PUT 1*:` resolves the whole function through the tree-sitter block walk.
    let block = run(&t.edit, json!({"input": format!("[lib.rs#{}]\nPUT 1*:\n+fn main() {{\n+    println!(\"hi\");\n+}}\n", tag(r, "lib.rs"))})).await;
    assert!(text(&block).contains("PUT 1*: → resolved lines 1-4 (4 lines)"), "{}", text(&block));
    assert_eq!(std::fs::read_to_string(r.join("lib.rs")).unwrap(), "fn main() {\n    println!(\"hi\");\n}\n");
    // Breaking the syntax is applied, with the engine warning and the host note.
    let broken =
        run(&t.edit, json!({"input": format!("[lib.rs#{}]\nPUT 2.=2:\n+    println!(\"hi\";\n", tag(r, "lib.rs"))}))
            .await;
    assert!(!broken.is_error);
    let tb = text(&broken);
    assert!(tb.contains("Warnings:\nThis edit introduced a syntax error near line 2"), "{tb}");
    assert!(tb.ends_with("<<>>\nWarning: lib.rs no longer parses after this edit. The change was applied; re-read the edited region and fix the syntax, or revert if unintended."), "{tb}");

    // Move, then delete.
    std::fs::write(r.join("a.py"), GREET).unwrap();
    let mv = run(&t.edit, json!({"input": format!("[a.py#{}]\nMV lib/greet.py\n", tag(r, "a.py"))})).await;
    assert!(text(&mv).starts_with("[lib/greet.py#") && text(&mv).contains("Moved to lib/greet.py"), "{}", text(&mv));
    assert!(!r.join("a.py").exists() && std::fs::read_to_string(r.join("lib/greet.py")).unwrap() == GREET);
    let rem = run(&t.edit, json!({"input": format!("[lib/greet.py#{}]\nREM\n", tag(r, "lib/greet.py"))})).await;
    assert_eq!(text(&rem), "Deleted lib/greet.py");
    assert!(!r.join("lib/greet.py").exists());

    // Cut and paste through a named register across sections.
    std::fs::write(r.join("x.py"), "a = 1\nb = 2\n").unwrap();
    std::fs::write(r.join("y.py"), "c = 3\n").unwrap();
    let cut = format!("[x.py#{}]\nCUT 1.=1 @v\n[y.py#{}]\nPUT >1 @v\n", tag(r, "x.py"), tag(r, "y.py"));
    let out = run(&t.edit, json!({"input": cut})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("x.py")).unwrap(), "b = 2\n");
    assert_eq!(std::fs::read_to_string(r.join("y.py")).unwrap(), "c = 3\na = 1\n");
    assert!(out.details.unwrap()["perFileResults"].as_array().is_some_and(|a| a.len() == 2));
}

#[tokio::test]
async fn patch_and_apply_patch_modes() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::write(r.join("app.py"), "def greet():\n    print('Hi')\n").unwrap();
    let t = tools(r, EditMode::Patch);
    let upd = run(&t.edit, json!({"path": "app.py", "edits": [{"op": "update", "diff": "@@ def greet():\n def greet():\n-    print('Hi')\n+    print('Hello')\n"}]})).await;
    assert!(!upd.is_error, "{}", text(&upd));
    assert_eq!(std::fs::read_to_string(r.join("app.py")).unwrap(), "def greet():\n    print('Hello')\n");
    let create = run(&t.edit, json!({"path": "new/hello.txt", "edits": [{"op": "create", "diff": "Hello\n"}]})).await;
    assert!(!create.is_error, "{}", text(&create));
    assert_eq!(std::fs::read_to_string(r.join("new/hello.txt")).unwrap(), "Hello\n");

    let t = tools(r, EditMode::ApplyPatch);
    let patch = "*** Begin Patch\n*** Update File: app.py\n*** Move to: src/main.py\n@@ def greet():\n-    print('Hello')\n+    print('Hello, world!')\n*** Delete File: new/hello.txt\n*** End Patch\n";
    let out = run(&t.edit, json!({"input": patch})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(!r.join("app.py").exists() && !r.join("new/hello.txt").exists());
    assert_eq!(std::fs::read_to_string(r.join("src/main.py")).unwrap(), "def greet():\n    print('Hello, world!')\n");
    let bad =
        run(&t.edit, json!({"input": "*** Begin Patch\n*** Update File: missing.py\n@@\n-a\n+b\n*** End Patch\n"}))
            .await;
    assert!(bad.is_error, "{}", text(&bad));
}

#[tokio::test]
async fn tool_surface_and_read_edge_cases() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let ctx = ToolContext::new(r).with_edit(EditMode::Hashline, true);
    let edit = EditTool::new(ctx.clone());
    let def = edit.definition();
    assert_eq!(def.name, "edit");
    assert_eq!(def.description, pi_edit::description(EditMode::Hashline));
    assert_eq!(def.parameters["required"], json!(["input"]));
    assert_eq!(edit.concurrency(&JsonObject::new()), Concurrency::Exclusive);
    let replace = EditTool::new(ToolContext::new(r).with_edit(EditMode::Replace, true));
    assert_eq!(replace.definition().parameters["required"], json!(["path", "old_string", "new_string"]));
    let names: Vec<String> = builtin_tools(ctx.clone()).iter().map(|t| t.definition().name).collect();
    assert_eq!(names, ["read", "write", "edit", "bash", "grep", "glob"]);
    // Hashline display needs the edit tool: disabled edit keeps plain reads.
    assert!(!ToolContext::new(r).with_edit(EditMode::Hashline, false).hashlines());
    assert!(ToolContext::new(r).hashlines(), "upstream default: edit tool exposed in hashline mode");

    let read = ReadTool { ctx: ctx.clone() };
    std::fs::write(r.join("a.py"), GREET).unwrap();
    let raw = text(&run(&read, json!({"path": "a.py:raw"})).await);
    assert_eq!(raw, GREET.trim_end(), "raw reads carry no anchors");
    let range = text(&run(&read, json!({"path": "a.py:2-3"})).await);
    assert!(range.starts_with(&format!("[a.py#{}]\n2:    msg", tag(r, "a.py"))), "{range}");
    assert!(range.ends_with("[1 more lines in file. Use :4 to continue]"), "{range}");
    let wide = format!("{}\n", "x".repeat(60 * 1024));
    std::fs::write(r.join("wide.txt"), &wide).unwrap();
    assert_eq!(
        text(&run(&read, json!({"path": "wide.txt"})).await),
        "[Line 1 is 60.0KB, exceeds 50.0KB limit. Hashline output requires full lines; cannot emit an editable numbered preview for a truncated line.]"
    );
    // A new file comes from `write`; hashline edits need an existing file.
    let write = WriteTool { ctx: ctx.clone() };
    run(&write, json!({"path": "n.txt", "content": "one\n"})).await;
    let out = run(&edit, json!({"input": format!("[n.txt#{}]\nPUT 1.=1:\n+uno\n", tag(r, "n.txt"))})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("n.txt")).unwrap(), "uno\n");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let e = edit.execute("c", args(json!({"input": "x"})), cancel, Arc::new(|_| {})).await.unwrap_err();
    assert_eq!(e.0, "Edit was aborted");
}

#[tokio::test]
async fn data_safety_refusals_and_partial_failure_report() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let t = tools(r, EditMode::Hashline);
    // Not valid UTF-8: refused before any write, bytes untouched.
    let latin1 = b"caf\xe9 au lait\nsecond\nthird\n".to_vec();
    std::fs::write(r.join("l1.txt"), &latin1).unwrap();
    let rt = tools(r, EditMode::Replace);
    let out = run(&rt.edit, json!({"path": "l1.txt", "old_string": "third", "new_string": "THIRD"})).await;
    assert!(out.is_error);
    assert!(text(&out).starts_with("Refusing to edit l1.txt: the file is not valid UTF-8 text"), "{}", text(&out));
    assert_eq!(std::fs::read(r.join("l1.txt")).unwrap(), latin1);

    // A move onto an existing different file is refused; both files intact.
    std::fs::write(r.join("a.txt"), "AAA\n").unwrap();
    std::fs::write(r.join("b.txt"), "BBB precious\n").unwrap();
    let out = run(&t.edit, json!({"input": format!("[a.txt#{}]\nMV b.txt\n", tag(r, "a.txt"))})).await;
    assert!(out.is_error);
    assert_eq!(text(&out), "Cannot move a.txt to b.txt: destination already exists.");
    assert_eq!(std::fs::read_to_string(r.join("b.txt")).unwrap(), "BBB precious\n");
    assert!(r.join("a.txt").exists());

    // A write failing after an earlier file landed names that file.
    std::fs::write(r.join("p1.txt"), "one\n").unwrap();
    std::fs::write(r.join("p2.txt"), "two\n").unwrap();
    std::fs::write(r.join("blocker"), "not a dir\n").unwrap();
    let payload =
        format!("[p1.txt#{}]\nPUT >1:\n+added\n[p2.txt#{}]\nMV blocker/p3.txt\n", tag(r, "p1.txt"), tag(r, "p2.txt"));
    let out = run(&t.edit, json!({"input": payload})).await;
    assert!(out.is_error);
    let msg = text(&out);
    assert!(msg.starts_with("Cannot write blocker/p3.txt:"), "{msg}");
    assert!(
        msg.ends_with("Already changed on disk by this edit before the failure (re-read before retrying): p1.txt"),
        "{msg}"
    );
    assert_eq!(std::fs::read_to_string(r.join("p1.txt")).unwrap(), "one\nadded\n");

    // A moved script keeps its executable bit.
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(r.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(r.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = run(&t.edit, json!({"input": format!("[run.sh#{}]\nMV bin/run.sh\n", tag(r, "run.sh"))})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::metadata(r.join("bin/run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
}

#[tokio::test]
async fn hashline_write_notebooks_and_untaggable_reads() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let ctx = ToolContext::new(r);
    let write = WriteTool { ctx: ctx.clone() };
    let read = ReadTool { ctx: ctx.clone() };
    let edit = EditTool::new(ctx.clone());
    // Copied read output: header path unwrapped, `N:` prefixes stripped, fresh tag returned.
    let out = run(&write, json!({"path": "[w.py#ABCD]", "content": "1:def f():\n2:    return 1\n"})).await;
    let body = "def f():\n    return 1\n";
    assert_eq!(std::fs::read_to_string(r.join("w.py")).unwrap(), body);
    assert!(!r.join("[w.py#ABCD]").exists());
    let fresh = pi_edit::store::file_hash(body);
    assert_eq!(
        text(&out),
        format!(
            "[w.py#{fresh}]\nSuccessfully wrote 22 bytes to w.py\nNote: auto-stripped hashline display prefixes from content before writing."
        )
    );
    // The returned tag anchors the next edit without a re-read.
    let out = run(&edit, json!({"input": format!("[w.py#{fresh}]\nPUT 2.=2:\n+    return 2\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("w.py")).unwrap(), "def f():\n    return 2\n");

    // Notebooks read as editable cells; the tag matches what edit sees.
    let nb = json!({
        "cells": [{"cell_type": "code", "execution_count": null, "metadata": {}, "outputs": [], "source": ["print(1)\n"]}],
        "metadata": {}, "nbformat": 4, "nbformat_minor": 5
    });
    std::fs::write(r.join("n.ipynb"), serde_json::to_string_pretty(&nb).unwrap()).unwrap();
    let shown = text(&run(&read, json!({"path": "n.ipynb"})).await);
    let header = shown.lines().next().unwrap().to_string();
    assert!(header.starts_with("[n.ipynb#") && !shown.contains("\"cells\""), "{shown}");
    let print_line = shown.lines().find(|l| l.ends_with(":print(1)")).unwrap().split(':').next().unwrap().to_string();
    let out = run(&edit, json!({"input": format!("{header}\nPUT {print_line}.={print_line}:\n+print(2)\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(std::fs::read_to_string(r.join("n.ipynb")).unwrap().contains("print(2)"));

    // Oversized first line keeps the continuation notice.
    std::fs::write(r.join("wide.txt"), format!("{}\nsecond\nthird\n", "x".repeat(60 * 1024))).unwrap();
    assert_eq!(
        text(&run(&read, json!({"path": "wide.txt"})).await),
        "[Line 1 is 60.0KB, exceeds 50.0KB limit. Hashline output requires full lines; cannot emit an editable numbered preview for a truncated line.]\n\n[2 more lines in file. Use :2 to continue]"
    );
    // Invalid UTF-8 past the sniff window: display-only rows, no tag.
    let mut bytes = "ok line\n".repeat(2000).into_bytes();
    bytes.extend_from_slice(b"bad \xff\n");
    std::fs::write(r.join("bin.txt"), bytes).unwrap();
    assert_eq!(
        text(&run(&read, json!({"path": "bin.txt:1-2"})).await),
        "1|ok line\n2|ok line\n\n[1999 more lines in file. Use :3 to continue]"
    );
}

#[tokio::test]
async fn engine_level_guards_cover_recovery_and_ordering() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    // A Latin-1 file reached through unique-suffix recovery is still refused.
    std::fs::create_dir_all(r.join("sub")).unwrap();
    let latin1 = b"caf\xe9\nthird\n".to_vec();
    std::fs::write(r.join("sub/legacy.txt"), &latin1).unwrap();
    let rt = tools(r, EditMode::Replace);
    let out = run(&rt.edit, json!({"path": "legacy.txt", "old_string": "third", "new_string": "THIRD"})).await;
    assert!(out.is_error && text(&out).starts_with("Refusing to edit sub/legacy.txt"), "{}", text(&out));
    assert_eq!(std::fs::read(r.join("sub/legacy.txt")).unwrap(), latin1);
    // Deleting such a file re-encodes nothing and stays allowed.
    let t = tools(r, EditMode::Hashline);
    let lossy_tag = pi_edit::store::file_hash(&String::from_utf8_lossy(&latin1));
    let out = run(&t.edit, json!({"input": format!("[sub/legacy.txt#{lossy_tag}]\nREM\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(!r.join("sub/legacy.txt").exists());

    // `REM b.txt` earlier in the payload frees the `MV` destination.
    std::fs::write(r.join("a.txt"), "new\n").unwrap();
    std::fs::write(r.join("b.txt"), "old\n").unwrap();
    let payload = format!("[b.txt#{}]\nREM\n[a.txt#{}]\nMV b.txt\n", tag(r, "b.txt"), tag(r, "a.txt"));
    let out = run(&t.edit, json!({"input": payload})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(std::fs::read_to_string(r.join("b.txt")).unwrap(), "new\n");
    assert!(!r.join("a.txt").exists());

    // Lone CR and BOM: displayed numbers are the engine's numbers.
    std::fs::write(r.join("cr.txt"), "a\rb\nc\n").unwrap();
    let shown = text(&run(&t.read, json!({"path": "cr.txt"})).await);
    assert!(shown.ends_with("\n1:a\n2:b\n3:c"), "{shown}");
    let header = shown.lines().next().unwrap().to_string();
    let out = run(&t.edit, json!({"input": format!("{header}\nPUT 3.=3:\n+C\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(std::fs::read_to_string(r.join("cr.txt")).unwrap().ends_with("C\n"), "the third line changed");
    std::fs::write(r.join("bom.txt"), "\u{feff}first\nsecond\n").unwrap();
    let shown = text(&run(&t.read, json!({"path": "bom.txt"})).await);
    assert!(shown.ends_with("\n1:first\n2:second"), "{shown:?}");
}

#[tokio::test]
async fn write_stripping_gate_and_notebook_tags() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let ctx = ToolContext::new(r);
    let write = WriteTool { ctx: ctx.clone() };
    let edit = EditTool::new(ctx);
    // Non-consecutive numbered rows are content, not read output.
    let yaml = "200: OK\n404: Not Found\n";
    let out = run(&write, json!({"path": "codes.yaml", "content": yaml})).await;
    assert_eq!(std::fs::read_to_string(r.join("codes.yaml")).unwrap(), yaml);
    assert!(!text(&out).contains("auto-stripped"));
    let out = run(&write, json!({"path": "standup.txt", "content": "10:30 standup"})).await;
    assert_eq!(std::fs::read_to_string(r.join("standup.txt")).unwrap(), "10:30 standup");
    assert!(!text(&out).contains("auto-stripped"));
    // A notebook write returns a tag of its editable cells, usable by edit.
    let nb = json!({
        "cells": [{"cell_type": "code", "execution_count": null, "metadata": {}, "outputs": [], "source": ["x = 1\n"]}],
        "metadata": {}, "nbformat": 4, "nbformat_minor": 5
    });
    let out = run(&write, json!({"path": "n.ipynb", "content": serde_json::to_string_pretty(&nb).unwrap()})).await;
    let header = text(&out).lines().next().unwrap().to_string();
    let cells =
        pi_edit::notebook::notebook_to_editable_text(&std::fs::read_to_string(r.join("n.ipynb")).unwrap(), "n.ipynb")
            .unwrap();
    let line = cells.lines().position(|l| l == "x = 1").unwrap() + 1;
    let out = run(&edit, json!({"input": format!("{header}\nPUT {line}.={line}:\n+x = 2\n")})).await;
    assert!(!out.is_error, "{}", text(&out));
    assert!(std::fs::read_to_string(r.join("n.ipynb")).unwrap().contains("x = 2"));
}
