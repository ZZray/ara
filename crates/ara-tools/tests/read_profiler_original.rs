//! Fixed OMP 596f2da cpuprofile.test.ts (9) and sample-profile.test.ts (12).
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Original IDs/assertions retained in coarse groups. READ gate fragment is
//! separately prepared by the glue owner and added only at union freeze.
use ara_agent::{AgentTool, ToolOutput};
use ara_ai::UserBlock;
use ara_rpc::json::{WireString, WireValue};
use ara_tools::{ToolContext, read::ReadTool, read_profile::*};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const BOX_MEASURE: &str = "_RNvNtCsfMEenOU8j5j_11slab_kernel6layout11box_measure";
fn utf8(text: WireString) -> String {
    text.to_utf8()
        .expect("original ordinary fixture is representable")
}
fn cpu_fixture() -> Value {
    let frame = |name: &str, url: Option<&str>, line: Option<u32>| {
        let mut value = json!({"functionName":name});
        if let Some(url) = url {
            value["url"] = json!(url);
        }
        if let Some(line) = line {
            value["lineNumber"] = json!(line);
        }
        value
    };
    let mut samples = Vec::new();
    for (id, count) in [(2, 2), (3, 5), (4, 30), (6, 20), (5, 25), (8, 3), (9, 15)] {
        samples.extend(std::iter::repeat_n(id, count));
    }
    json!({"nodes":[
        {"id":1,"callFrame":frame("(root)",None,None),"hitCount":0,"children":[2,8,9]},
        {"id":2,"callFrame":frame("main",Some("file:///work/app/src/index.ts"),Some(9)),"hitCount":2,"children":[3,5]},
        {"id":3,"callFrame":frame("render",Some("file:///work/app/src/render.ts"),Some(1)),"hitCount":5,"children":[4]},
        {"id":4,"callFrame":frame("layout",Some("file:///work/app/src/render.ts"),Some(5)),"hitCount":30,"children":[6]},
        {"id":5,"callFrame":frame("parse",Some("file:///work/app/src/parse.ts"),Some(3)),"hitCount":25,"children":[]},
        {"id":6,"callFrame":frame("layout",Some("file:///work/app/src/render.ts"),Some(5)),"hitCount":20,"children":[]},
        {"id":8,"callFrame":frame("(garbage collector)",None,None),"hitCount":3,"children":[]},
        {"id":9,"callFrame":frame("(idle)",None,None),"hitCount":15,"children":[]}],
        "startTime":1000,"endTime":101000,"samples":samples,"timeDeltas":vec![1000;100]})
}
fn sample_fixture() -> String {
    [
        "Analysis of sampling demo (pid 42) every 1 millisecond".to_owned(),
        "Process:         demo [42]".into(),
        "Path:            /tmp/demo".into(),
        "Physical footprint:         10M".into(),
        "Physical footprint (peak):  12M".into(),
        "----".into(),
        "".into(),
        "Call graph:".into(),
        "    100 Thread_1: main".into(),
        "    + 100 start  (in dyld) + 8  [0x100]".into(),
        "    +   100 run  (in demo) + 24  [0x200]".into(),
        format!("    +     70 {BOX_MEASURE}  (in demo) + 8  [0x300]"),
        format!("    +     ! 30 {BOX_MEASURE}  (in demo) + 12,16,...  [0x301,0x302,...]"),
        "    +     !   10 memcpy  (in libsystem_platform.dylib) + 4  [0x400]".into(),
        "    +     12 alloc  (in demo) + 8  [0x500]".into(),
        "    +     8 alloc  (in demo) + 40  [0x510]".into(),
        "    +     10 semaphore_wait_trap  (in libsystem_kernel.dylib) + 8  [0x600]".into(),
        "    50 Thread_2: idler".into(),
        "    + 50 thread_start  (in libsystem_pthread.dylib) + 8  [0x700]".into(),
        "    +   50 mach_msg2_trap  (in libsystem_kernel.dylib) + 8  [0x800]".into(),
        "".into(),
        "Total number in stack (recursive counted multiple, when >=5):".into(),
        "        70       something  (in demo) + 0  [0x300]".into(),
        "".into(),
        "Binary Images:".into(),
        "       0x100 -        0x200 +demo (0) <UUID> /tmp/demo".into(),
        "".into(),
    ]
    .join("\n")
}
fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| {
            if let UserBlock::Text(block) = block {
                Some(block.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
async fn actual_read(ctx: &ToolContext, path: &str) -> ToolOutput {
    ReadTool { ctx: ctx.clone() }
        .execute(
            "read-profiler",
            json!({"path":path}).as_object().unwrap().clone(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn original_cpu_profile_nine_group() {
    let fixture = cpu_fixture();
    let source = fixture.to_string();
    // CPU-01: accepts bare and CDP wrapper.
    assert_eq!(parse_cpu_profile(&source).unwrap().nodes.len(), 8);
    assert_eq!(
        parse_cpu_profile(&json!({"profile":fixture}).to_string())
            .unwrap()
            .samples
            .unwrap()
            .len(),
        100
    );
    // CPU-02: rejects structurally invalid profile text, unchanged four inputs.
    for source in [
        "not json".to_owned(),
        json!({"nodes":[],"startTime":0,"endTime":1}).to_string(),
        json!({"nodes":[{"id":1}],"startTime":0,"endTime":1}).to_string(),
        json!({"hello":"world"}).to_string(),
    ] {
        assert!(parse_cpu_profile(&source).is_none());
    }
    let rendered = utf8(
        render_cpu_profile(&source)
            .unwrap()
            .expect("fixture must render"),
    );
    // CPU-03: wall/sample/onCPU totals, idle excluded.
    assert!(
        rendered.contains("V8 CPU profile: 0.10 s wall clock, 100 samples (avg interval 1000 µs)")
    );
    assert!(rendered.contains("On-CPU total: 0.09 s (85.0% of wall clock)"));
    // CPU-04: recursion and URL shortening.
    assert!(rendered.contains("layout (…/app/src/render.ts:6) [recursive ×2]"));
    assert!(!rendered.contains("file://"));
    // CPU-05: synthetic root promoted, GC retained, idle excluded.
    let hot = &rendered
        [rendered.find("## Hot paths").unwrap()..rendered.find("## Top functions").unwrap()];
    assert!(!hot.contains("(root)"));
    assert!(hot.contains("    82.0  96.5%  main (…/app/src/index.ts:10)"));
    assert!(hot.contains("(garbage collector)"));
    assert!(!hot.contains("(idle)"));
    // CPU-06: exact original self-ranking and recursive aggregate.
    let section = &rendered[rendered.find("## Top functions").unwrap()..];
    let order: Vec<_> = ["layout", "parse", "render"]
        .into_iter()
        .map(|symbol| section.find(&format!("  {symbol} (")).unwrap())
        .collect();
    assert!(*order.iter().min().unwrap() > 0);
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
    assert!(section.contains("    50.0  58.8%  layout (…/app/src/render.ts:6)"));
    assert!(!section.contains("(idle)"));
    // CPU-07: absent streams use hitCount.
    let mut bare = cpu_fixture();
    bare.as_object_mut().unwrap().remove("samples");
    bare.as_object_mut().unwrap().remove("timeDeltas");
    let from_hits = utf8(
        render_cpu_profile(&bare.to_string())
            .unwrap()
            .expect("hitCount fixture must render"),
    );
    assert!(from_hits.contains("On-CPU total: 0.09 s (85.0% of wall clock)"));
    assert!(from_hits.contains("    50.0  58.8%  layout (…/app/src/render.ts:6)"));
    assert!(!from_hits.contains("samples (avg interval"));
    // CPU-08: actual ReadTool summary, original :raw escape.
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path());
    let file = dir.path().join("app.cpuprofile");
    std::fs::write(&file, &source).unwrap();
    let summary = text(&actual_read(&ctx, &file.to_string_lossy()).await);
    assert!(summary.contains("V8 CPU profile"));
    assert!(summary.contains("layout (…/app/src/render.ts:6) [recursive ×2]"));
    assert!(!summary.contains("\"timeDeltas\""));
    let raw = text(&actual_read(&ctx, &format!("{}:raw", file.display())).await);
    assert!(raw.contains("\"timeDeltas\""));
    // CPU-09: invalid profile extension falls back to real plain file.
    let notes = dir.path().join("notes.cpuprofile");
    std::fs::write(&notes, "these are just notes\nsecond line\n").unwrap();
    assert!(
        text(&actual_read(&ctx, &notes.to_string_lossy()).await).contains("these are just notes")
    );
}

#[tokio::test]
async fn original_sample_profile_twelve_group() {
    // SAMPLE-01: all three original Rust-v0 examples.
    for (raw, want) in [
        (BOX_MEASURE, "slab_kernel::layout::box_measure"),
        (
            "_RNvNtNtNtNtNtCslWr2rddMglm_4core3num3imp7flt2dec8strategy6dragon12format_exact",
            "core::num::imp::flt2dec::strategy::dragon::format_exact",
        ),
        (
            "_RNvMs_NtCsaeZfelr0jAd_5alloc3vecINtB4_3VechE7reserveCscFRsxFFybUw_9addr2line",
            "alloc::vec::Vec::reserve::addr2line",
        ),
    ] {
        assert_eq!(utf8(demangle_symbol(&raw.into())), want);
    }
    // SAMPLE-02: legacy and hash removal.
    assert_eq!(
        utf8(demangle_symbol(
            &"_ZN4core3fmt9Formatter3pad17h1c9860dbd7c7fc47E".into()
        )),
        "core::fmt::Formatter::pad"
    );
    // SAMPLE-03: both original non-Rust pass-through examples.
    for raw in ["-[NSApplication run]", "_platform_memmove"] {
        assert_eq!(utf8(demangle_symbol(&raw.into())), raw);
    }
    // SAMPLE-04: header, thread roots and decorated offset tree.
    let source = sample_fixture();
    let profile = parse_sample_profile(&source).unwrap();
    assert_eq!(utf8(profile.header.process.clone()), "demo");
    assert_eq!(profile.header.pid, 42.0);
    assert_eq!(profile.header.interval_ms, 1.0);
    assert_eq!(utf8(profile.header.path.clone().unwrap()), "/tmp/demo");
    assert_eq!(
        profile
            .threads
            .iter()
            .map(|thread| (
                utf8(thread.id.clone()),
                utf8(thread.name.clone().unwrap()),
                thread.total
            ))
            .collect::<Vec<_>>(),
        vec![
            ("1".into(), "main".into(), 100.0),
            ("2".into(), "idler".into(), 50.0)
        ]
    );
    let run = &profile.threads[0].roots[0].children[0];
    assert_eq!(utf8(run.symbol.clone()), "run");
    assert_eq!(utf8(run.module.clone().unwrap()), "demo");
    assert_eq!(
        run.children
            .iter()
            .map(|child| child.count)
            .collect::<Vec<_>>(),
        vec![70.0, 12.0, 8.0, 10.0]
    );
    assert_eq!(
        utf8(run.children[0].children[0].symbol.clone()),
        BOX_MEASURE
    );
    // SAMPLE-05: both original rejected reports.
    assert!(parse_sample_profile("just some notes\nCall graph:\n").is_none());
    assert!(parse_sample_profile("").is_none());
    let rendered = utf8(render_sample_profile(&source).expect("fixture must render"));
    // SAMPLE-06: blocked samples excluded.
    assert!(rendered.contains("main (Thread_1) — 100 samples, 90 on-CPU (90.0%)"));
    // SAMPLE-07: idle thread exact classification.
    assert!(rendered.contains("idler (Thread_2): blocked in mach_msg2_trap (0 on-CPU)"));
    assert!(!rendered.contains("## idler"));
    // SAMPLE-08: recursion/demangling.
    assert!(rendered.contains("slab_kernel::layout::box_measure [recursive ×2]"));
    assert!(!rendered.contains(BOX_MEASURE));
    // SAMPLE-09: same symbol offset siblings merge to exactly one20 row.
    let tree = &rendered[..rendered.find("## Idle").unwrap()];
    let alloc: Vec<_> = tree.lines().filter(|line| line.contains("alloc")).collect();
    assert_eq!(
        alloc
            .into_iter()
            .filter(|line| regex::Regex::new(r"^\s*20\b").unwrap().is_match(line))
            .count(),
        1
    );
    // SAMPLE-10: process-wide order and no wait functions.
    let section = &rendered[rendered.find("## Top functions").unwrap()..];
    let order: Vec<_> = ["slab_kernel::layout::box_measure", "alloc", "memcpy"]
        .into_iter()
        .map(|symbol| section.find(symbol).unwrap())
        .collect();
    assert!(*order.iter().min().unwrap() > 0);
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
    assert!(!section.contains("semaphore_wait_trap"));
    assert!(!section.contains("mach_msg2_trap"));
    // SAMPLE-11: actual summary and raw READ bodies.
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path());
    let file = dir.path().join("demo-42.sample.txt");
    std::fs::write(&file, &source).unwrap();
    let summary = text(&actual_read(&ctx, &file.to_string_lossy()).await);
    assert!(summary.contains("macOS sample profile: demo (pid 42)"));
    assert!(summary.contains("slab_kernel::layout::box_measure"));
    assert!(!summary.contains(BOX_MEASURE));
    let raw = text(&actual_read(&ctx, &format!("{}:raw", file.display())).await);
    assert!(raw.contains("Call graph:"));
    assert!(raw.contains(BOX_MEASURE));
    // SAMPLE-12: invalid wearing extension remains plain file.
    let notes = dir.path().join("notes.sample.txt");
    std::fs::write(&notes, "these are just notes\nsecond line\n").unwrap();
    assert!(
        text(&actual_read(&ctx, &notes.to_string_lossy()).await).contains("these are just notes")
    );
}

fn metric(value: f64) -> WireString {
    ara_prompt::js::f64_to_string(value).into()
}
fn node(key: &str, value: f64, children: Vec<ProfileNode>) -> ProfileNode {
    ProfileNode {
        key: key.into(),
        label: key.into(),
        value,
        recursion: 0,
        children,
    }
}
#[test]
fn original_profile_tree_source_group() {
    let mut a = node("a", 4.0, vec![node("x", 3.0, vec![])]);
    let mut b = node(
        "a",
        5.0,
        vec![node("x", 2.0, vec![]), node("y", 1.0, vec![])],
    );
    b.recursion = 2;
    merge_into(&mut a, b);
    assert_eq!(a.value, 9.0);
    assert_eq!(a.recursion, 2);
    assert_eq!(
        a.children
            .iter()
            .map(|row| (&row.key, row.value))
            .collect::<Vec<_>>(),
        vec![(&WireString::from("x"), 5.0), (&WireString::from("y"), 1.0)]
    );
    let mut recursive = node(
        "a",
        10.0,
        vec![node(
            "a",
            8.0,
            vec![node("a", 6.0, vec![node("leaf", 5.0, vec![])])],
        )],
    );
    flatten_recursion(&mut recursive);
    assert_eq!(recursive.value, 10.0);
    assert_eq!(recursive.recursion, 2);
    assert_eq!(recursive.children[0].key, WireString::from("leaf"));
    assert_eq!(utf8(format_pct(1.0, 0.0)), "0%");
    assert_eq!(utf8(format_pct(1.0, 8.0)), "12.5%");
    assert_eq!(utf8(format_pct(f64::NAN, 1.0)), "NaN%");
    assert_eq!(utf8(format_pct(1.25, 100.0)), "1.3%");
    assert_eq!(utf8(format_pct(-1.25, 100.0)), "-1.3%");
    assert_eq!(utf8(format_pct(-0.0, 100.0)), "0.0%");
    assert_eq!(utf8(format_pct(-1e-10, 100.0)), "-0.0%");
    let mut ctx = RenderTreeContext {
        out: Vec::new(),
        total: 10.0,
        min_value: 1.0,
        format_value: metric,
        value_width: 6,
    };
    let mut chain = node(
        "a",
        10.0,
        vec![node(
            "b",
            10.0,
            vec![node(
                "c",
                10.0,
                vec![node("d", 10.0, vec![node("e", 10.0, vec![])])],
            )],
        )],
    );
    render_profile_node(&mut chain, 0, &mut ctx);
    assert_eq!(utf8(ctx.out.remove(0)), "    10 100.0%  a › ⋯3 frames⋯ › e");
    // PRE-P-01: infinity-infinity => NaN; original own>=min is false and
    // retains pass-through collapsing, rather than own<min stopping it.
    let mut inf = node("a", f64::INFINITY, vec![node("b", f64::INFINITY, vec![])]);
    ctx.total = f64::INFINITY;
    render_profile_node(&mut inf, 0, &mut ctx);
    assert!(utf8(ctx.out.remove(0)).ends_with("a › b"));
    let mut cutoff = node("long", 10.0, vec![]);
    let mut units = vec![65; 158];
    units.extend([0xd83d, 0xde00, 66]);
    cutoff.label = WireString::from_units(units);
    ctx.total = 10.0;
    render_profile_node(&mut cutoff, 0, &mut ctx);
    let out = ctx.out.remove(0);
    assert!(out.units().ends_with(&[0xd83d, 0x2026]));
    assert!(out.to_utf8().is_err());
    let mut border = node(
        "root",
        10.0,
        vec![node("keep", 9.0, vec![]), node("prune", 0.5, vec![])],
    );
    render_profile_node(&mut border, 0, &mut ctx);
    assert!(utf8(ctx.out.remove(0)).ends_with("root"));
    assert!(utf8(ctx.out.remove(0)).ends_with("keep"));
    assert!(ctx.out.is_empty());
    let mut ties = node(
        "root",
        10.0,
        vec![node("first", 4.0, vec![]), node("second", 4.0, vec![])],
    );
    render_profile_node(&mut ties, 0, &mut ctx);
    let lines: Vec<_> = ctx.out.drain(..).map(utf8).collect();
    assert!(lines[1].ends_with("first"));
    assert!(lines[2].ends_with("second"));
    assert_eq!(ties.children[0].key, WireString::from("first"));
}
#[test]
fn original_cpu_profile_source_edges_group() {
    assert!(is_cpu_profile_path("x.CPUPROFILE"));
    assert!(!is_cpu_profile_path("x.cpuprofile\n"));
    for source in [
        r#"{"nodes":null,"profile":{"nodes":[{"id":1,"callFrame":{"functionName":"x"},"hitCount":1}],"startTime":0,"endTime":1000}}"#,
        r#"{"nodes":[{"id":"1","callFrame":{"functionName":"x"}}],"startTime":0,"endTime":1}"#,
    ] {
        assert!(parse_cpu_profile(source).is_none());
    }
    let minimal = |nodes: Value, samples: Value, deltas: Value| json!({"nodes":nodes,"startTime":0,"endTime":10000,"samples":samples,"timeDeltas":deltas});
    let nodes = json!([{"id":1,"callFrame":{"functionName":"x"},"hitCount":5}]);
    // Chosen empty delta stream never falls back to hits.
    assert!(
        render_cpu_profile(&minimal(nodes.clone(), json!([1]), json!([])).to_string())
            .unwrap()
            .is_none()
    );
    let valid = minimal(
        nodes.clone(),
        json!([1, 1, 1, 1]),
        json!([0, -1, "bad", 1000]),
    );
    let result = utf8(render_cpu_profile(&valid.to_string()).unwrap().unwrap());
    assert!(result.contains("1.0"));
    assert!(result.contains("4 samples"));
    for field in [json!(123), json!(false), json!({"child":1})] {
        let invalid = minimal(
            json!([{"id":1,"callFrame":{"functionName":"x"},"children":field,"hitCount":1}]),
            Value::Null,
            Value::Null,
        );
        assert!(parse_cpu_profile(&invalid.to_string()).is_some());
        assert_eq!(
            render_cpu_profile(&invalid.to_string()).unwrap_err().kind,
            "TypeError"
        );
    }
    let invalid_url = minimal(
        json!([{"id":1,"callFrame":{"functionName":"x","url":true},"hitCount":1}]),
        Value::Null,
        Value::Null,
    );
    assert_eq!(
        render_cpu_profile(&invalid_url.to_string())
            .unwrap_err()
            .kind,
        "TypeError"
    );
    let loose = minimal(
        json!([{"id":1,"callFrame":{"functionName":"","url":"/a/node_modules/p/file.js","lineNumber":0},"hitCount":"2"}]),
        Value::Null,
        Value::Null,
    );
    let loose = utf8(render_cpu_profile(&loose.to_string()).unwrap().unwrap());
    assert!(loose.contains("(anonymous) (node_modules/p/file.js:1)"));
    let cycle = minimal(
        json!([{"id":1,"callFrame":{"functionName":"root"},"children":[2,999],"hitCount":1},{"id":2,"callFrame":{"functionName":"child"},"children":[1],"hitCount":1}]),
        Value::Null,
        Value::Null,
    );
    assert!(render_cpu_profile(&cycle.to_string()).unwrap().is_none());
    let surrogate = r#"{"nodes":[{"id":1,"callFrame":{"functionName":"\ud800"},"hitCount":1}],"startTime":0,"endTime":1000}"#;
    let result = render_cpu_profile(surrogate).unwrap().unwrap();
    assert!(result.units().contains(&0xd800));
    assert!(result.to_utf8().is_err());
    let overflow = r#"{"nodes":[{"id":1,"callFrame":{"functionName":"x"},"hitCount":1}],"startTime":0,"endTime":1e400}"#;
    assert!(parse_cpu_profile(overflow).unwrap().end_time.is_infinite());
    assert!(render_cpu_profile(overflow).unwrap().is_some());
    // Unknown sample IDs are counted in sample headers but not priced as a node.
    assert!(
        render_cpu_profile(&minimal(nodes, json!(["1", 999]), json!([1000, 1000])).to_string())
            .unwrap()
            .is_none()
    );
    assert!(matches!(WireValue::parse("null").unwrap(), WireValue::Null));
    // Full URL branches, frame labels, nullish children and line-number gates.
    for (url, want) in [
        ("file:///a/b.js", "x (/a/b.js:3)"),
        ("node_modules/p/f.js", "x (node_modules/p/f.js:3)"),
        ("/a/b/c/d/e.js", "x (…/c/d/e.js:3)"),
    ] {
        let value = minimal(
            json!([{"id":1,"callFrame":{"functionName":"x","url":url,"lineNumber":2},"children":null,"hitCount":1}]),
            Value::Null,
            Value::Null,
        );
        assert!(utf8(render_cpu_profile(&value.to_string()).unwrap().unwrap()).contains(want));
    }
    let many: Vec<Value> = (0..21)
        .map(|id| json!({"id":id,"callFrame":{"functionName":format!("fn{id:02}")},"hitCount":1}))
        .collect();
    let rendered = utf8(
        render_cpu_profile(&minimal(json!(many), Value::Null, Value::Null).to_string())
            .unwrap()
            .unwrap(),
    );
    let top = &rendered[rendered.find("## Top functions").unwrap()..];
    assert!(top.contains("fn00") && top.contains("fn19"));
    assert!(!top.contains("fn20"));
    assert!(top.find("fn00").unwrap() < top.find("fn01").unwrap());
    let no_path = minimal(
        json!([{"id":1,"callFrame":{"functionName":"x"}}]),
        json!([1]),
        json!([1]),
    );
    assert!(
        utf8(render_cpu_profile(&no_path.to_string()).unwrap().unwrap())
            .contains("(no call path above")
    );
    let duplicate = minimal(
        json!([{"id":1,"callFrame":{"functionName":"first"},"hitCount":1},{"id":1,"callFrame":{"functionName":"last"},"hitCount":2}]),
        Value::Null,
        Value::Null,
    );
    let rendered = utf8(render_cpu_profile(&duplicate.to_string()).unwrap().unwrap());
    let hot = &rendered
        [rendered.find("## Hot paths").unwrap()..rendered.find("## Top functions").unwrap()];
    assert!(hot.contains("first") && !hot.contains("last"));
    assert!(rendered[rendered.find("## Top functions").unwrap()..].contains("last"));
    let malformed_hit = minimal(
        json!([{"id":1,"callFrame":{"functionName":"x"},"hitCount":{"toString":null}}]),
        Value::Null,
        Value::Null,
    );
    assert_eq!(
        render_cpu_profile(&malformed_hit.to_string())
            .unwrap_err()
            .kind,
        "TypeError"
    );
    let idle_parent = minimal(
        json!([{"id":1,"callFrame":{"functionName":"(idle)"},"children":[2],"hitCount":2},{"id":2,"callFrame":{"functionName":"x"},"hitCount":1}]),
        Value::Null,
        Value::Null,
    );
    assert!(
        render_cpu_profile(&idle_parent.to_string())
            .unwrap()
            .is_some()
    );
    let decimal = minimal(
        json!([{"id":1,"callFrame":{"functionName":"x"}}]),
        json!([1]),
        json!([1250]),
    );
    assert!(
        utf8(render_cpu_profile(&decimal.to_string()).unwrap().unwrap()).contains("1.3 100.0%  x")
    );
    let mut fixed_two = minimal(
        json!([{"id":1,"callFrame":{"functionName":"x"},"hitCount":1}]),
        Value::Null,
        Value::Null,
    );
    fixed_two["endTime"] = json!(1125000);
    assert!(
        utf8(render_cpu_profile(&fixed_two.to_string()).unwrap().unwrap())
            .contains("V8 CPU profile: 1.13 s wall clock")
    );
    let mut rounded = minimal(
        json!([{"id":1,"callFrame":{"functionName":"x"}}]),
        json!([1, 1]),
        json!([1, 1]),
    );
    rounded["endTime"] = json!(3);
    assert!(
        utf8(render_cpu_profile(&rounded.to_string()).unwrap().unwrap())
            .contains("avg interval 2 µs")
    );
    // C07: a reachable cycle AND shared child must use one global visited set.
    // Fixed cpuprofile.ts build order is root1 -> A2 -> B3 (A2 skipped) -> leaf4;
    // root1's second B3 edge is then skipped. Self10/20/40 ms => one70 ms root.
    let cycle_and_diamond = json!({
        "nodes": [
            {"id":1,"callFrame":{"functionName":"(root)"},"hitCount":0,"children":[2,3]},
            {"id":2,"callFrame":{"functionName":"A"},"hitCount":10,"children":[3,4]},
            {"id":3,"callFrame":{"functionName":"B"},"hitCount":20,"children":[2]},
            {"id":4,"callFrame":{"functionName":"leaf"},"hitCount":40,"children":[]}
        ],
        "startTime":0,"endTime":70000
    });
    let rendered = utf8(render_cpu_profile(&cycle_and_diamond.to_string()).unwrap().unwrap());
    assert!(rendered.contains("On-CPU total: 0.07 s (100.0% of wall clock)"));
    let hot = rendered.split("\n## Hot paths\n").nth(1).unwrap()
        .split("\n\n## Top functions").next().unwrap();
    assert_eq!(hot, "    70.0 100.0%  A\n    40.0  57.1%    leaf\n    20.0  28.6%    B");
    let top = rendered.split("\n## Top functions by self time (idle time excluded)\n")
        .nth(1).unwrap().split("\n\n[Summarized view").next().unwrap();
    assert_eq!(top, "    40.0  57.1%  leaf\n    20.0  28.6%  B\n    10.0  14.3%  A");
    assert_eq!(hot.lines().filter(|line| line.contains("leaf")).count(), 1);
}
#[test]
fn original_sample_profile_source_edges_group() {
    assert!(is_sample_profile_path("x.SAMPLE.TXT"));
    assert!(!is_sample_profile_path("x.sample.txt\n"));
    let original = sample_fixture();
    assert!(parse_sample_profile(&format!("\u{feff}{original}")).is_none());
    assert!(parse_sample_profile(&original.replace('\n', "\r\n")).is_none());
    assert!(
        parse_sample_profile("Analysis of sampling x (pid 1) every 1 millisecond\nCall graph:\n")
            .is_none()
    );
    for raw in [
        "_R",
        "_ZN999shortE",
        "_ZN17h0123456789abcdefE",
        "_platform_memmove",
    ] {
        assert_eq!(demangle_symbol(&raw.into()), WireString::from(raw));
    }
    assert_eq!(utf8(demangle_symbol(&"__ZN4core3fmtE".into())), "core::fmt");
    assert_eq!(utf8(demangle_symbol(&"_ZN4a..bE".into())), "a::b");
    assert_eq!(utf8(demangle_symbol(&"_ZN4$LT$E".into())), "<");
    let idle = |symbol: &str| {
        format!(
            "Analysis of sampling x (pid 1) every 1 millisecond\nCall graph:\n    100 Thread_1:\n    + 100 {symbol}\n"
        )
    };
    for symbol in [
        "__accept",
        "__psynch_cvwait",
        "__psynch_mutexwait",
        "__psynch_rw_rdlock",
        "__psynch_rw_wrlock",
        "__recvfrom",
        "__select",
        "__semwait_signal",
        "__sigwait",
        "__ulock_wait",
        "__ulock_wait2",
        "__wait4",
        "__workq_kernreturn",
        "kevent",
        "kevent64",
        "kevent_qos",
        "mach_msg2_trap",
        "mach_msg_trap",
        "poll",
        "semaphore_timedwait_trap",
        "semaphore_wait_trap",
        "start_wqthread",
        "swtch_pri",
        "thread_suspend",
        "usleep",
        "constructor",
        "toString",
        "__proto__",
    ] {
        let rendered = utf8(render_sample_profile(&idle(symbol)).unwrap());
        assert!(rendered.contains(&format!("blocked in {symbol} (0 on-CPU)")));
        assert!(!rendered.contains("## Top functions"));
    }
    let classified = utf8(render_sample_profile(&idle("active")).unwrap());
    assert!(classified.contains("## Thread_1 — 100 samples, 100 on-CPU (100.0%)"));
    let header = original.replacen(
        "----",
        "Code Type:       ARM-64\nOS Version:      macOS 15.0\nUnknown Key:     ignored",
        1,
    );
    let rendered = utf8(render_sample_profile(&header).unwrap());
    assert!(rendered.contains("/tmp/demo | ARM-64 | macOS 15.0"));
    assert!(rendered.contains("Footprint: 10M (peak 12M)"));
    assert!(!rendered.contains("ignored"));
    let no_wait = "Analysis of sampling x (pid 1) every 2 milliseconds\nCall graph:\n    100 Thread_1: tiny\n    + 9 active\nSort by top of stack\n    100 Thread_2: ignored\n";
    let rendered = utf8(render_sample_profile(no_wait).unwrap());
    assert!(rendered.contains("tiny (Thread_1): 9/100 samples on-CPU"));
    assert!(!rendered.contains("Thread_2"));
    let threshold = "Analysis of sampling x (pid 1) every 1 millisecond\nCall graph:\n    5000 Thread_1: threshold\n    + 10 active\n";
    assert!(utf8(render_sample_profile(threshold).unwrap()).contains("## threshold"));
    let clipped = "Analysis of sampling x (pid 1) every 1 millisecond\nCall graph:\n    100 Thread_1: before\rignored\n    + 100 active\rignored\n";
    assert!(render_sample_profile(clipped).is_none());
    let escapes = "$LT$$GT$$RF$$BP$$C$$u20$$u27$$u7b$$u7d$..";
    assert_eq!(
        utf8(demangle_symbol(
            &format!("_ZN{}{escapes}E", escapes.len()).into()
        )),
        "<>&*, '{}::"
    );
    let waits = "Analysis of sampling x (pid 1) every 1 millisecond\nCall graph:\n    20 Thread_a\n    + 20 parent\n    +   10 __accept\n    +   10 poll\nBinary Images:\n    100 Thread_ignored\n";
    let rendered = utf8(render_sample_profile(waits).unwrap());
    assert!(rendered.contains("blocked in __accept (0 on-CPU)"));
    assert!(!rendered.contains("Thread_ignored"));
    let merged = "Analysis of sampling x (pid 1) every 1 millisecond\nCall graph:\n    20 Thread_a: first   name\n    + 20 parent\n    +   10 hot  (in first_module) + 1\n    +   10 hot  (in second_module) + 2\n";
    let rendered = utf8(render_sample_profile(merged).unwrap());
    assert!(rendered.contains("first name (Thread_a)"));
    assert!(rendered.contains("hot (first_module)"));
    assert!(!rendered.contains("second_module"));
    let peak_only = idle("active").replacen(
        "Call graph:",
        "Physical footprint (peak): 12M\nCall graph:",
        1,
    );
    assert!(!utf8(render_sample_profile(&peak_only).unwrap()).contains("Footprint"));
    let duration = "Analysis of sampling x (pid 1) every 1 millisecond\nCall graph:\n    1250 Thread_1\n    + 1250 active\n";
    assert!(utf8(render_sample_profile(duration).unwrap()).contains("Duration: ~1.3 s"));
    // S12/S15: process totals merge the same symbol across threads and retain
    // its FIRST module, independently of descending active-thread order.
    let cross_thread = concat!(
        "Analysis of sampling demo (pid 1) every 1 millisecond\nCall graph:\n",
        "    40 Thread_one: slower\n    + 40 shared  (in first_module) + 1\n",
        "    60 Thread_two: faster\n    + 60 shared  (in second_module) + 2\n"
    );
    let rendered = utf8(render_sample_profile(cross_thread).unwrap());
    assert!(rendered.contains("Process total: 100 on-CPU samples across 2 threads."));
    assert!(rendered.find("## faster (Thread_two)").unwrap()
        < rendered.find("## slower (Thread_one)").unwrap());
    let top = rendered.split("\n## Top functions by self samples (process-wide, blocked time excluded)\n")
        .nth(1).unwrap().split("\n\n[Summarized view").next().unwrap();
    assert_eq!(top, "   100 100.0%  shared (first_module)");
    assert!(!top.contains("second_module"));

    // S12/S15:21 tied symbols, each10 self samples in EACH of two threads.
    // Aggregate20 each/420 total=4.7619..=>4.8%; stable Top20 is fn00..fn19.
    // First modules win even when the second thread supplies other modules.
    let mut top_twenty = String::from(
        "Analysis of sampling demo (pid 1) every 1 millisecond\nCall graph:\n"
    );
    for (thread, module_prefix) in [("one", "first"), ("two", "second")] {
        top_twenty.push_str(&format!("    210 Thread_{thread}\n    + 210 batch  (in demo) + 0\n"));
        for index in 0..21 {
            let module = if index == 0 { format!("{module_prefix}_module") }
                else if thread == "one" { "demo".to_owned() } else { "other_module".to_owned() };
            top_twenty.push_str(&format!("    +   10 fn{index:02}  (in {module}) + 1\n"));
        }
    }
    let rendered = utf8(render_sample_profile(&top_twenty).unwrap());
    assert!(rendered.contains("Process total: 420 on-CPU samples across 2 threads."));
    let top = rendered.split("\n## Top functions by self samples (process-wide, blocked time excluded)\n")
        .nth(1).unwrap().split("\n\n[Summarized view").next().unwrap();
    let expected: Vec<String> = (0..20).map(|index| format!(
        "    20   4.8%  fn{index:02}{}", if index == 0 { " (first_module)" } else { "" }
    )).collect();
    assert_eq!(top.lines().collect::<Vec<_>>(), expected.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(rendered.contains("fn20"));
    assert!(!top.contains("fn20") && !top.contains("second_module") && !top.contains("other_module"));
}

#[tokio::test]
async fn original_read_profile_gate_and_selector_group() {
    // D01-D07 source branches, kept in the sixth coarse profiler group.
    use ara_tools::read::ReadDelimitedDispatcher;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Capture { outputs: Mutex<Vec<WireValue>>, errors: Mutex<Vec<String>> }
    #[async_trait::async_trait]
    impl ReadDelimitedDispatcher for Capture {
        async fn read_parts(&self, _: Vec<String>, _: CancellationToken) -> Result<ToolOutput, ara_agent::ToolError> {
            Err(ara_agent::ToolError("gate fixture has no compound request".into()))
        }
        async fn publish_in_memory_output(&self, output: WireValue) -> Result<(), ara_agent::ToolError> {
            self.outputs.lock().unwrap().push(output); Ok(())
        }
        async fn publish_profile_type_error(&self, message: String) -> Result<(), ara_agent::ToolError> {
            self.errors.lock().unwrap().push(message); Ok(())
        }
    }
    fn field<'a>(value: &'a WireValue, key: &str) -> Option<&'a WireValue> {
        let WireValue::Object(items) = value else { return None };
        items.iter().find(|(name, _)| name.equals_ascii(key)).map(|(_, value)| value)
    }
    fn wire_text(value: &WireValue) -> &WireString {
        let Some(WireValue::Array(content)) = field(value, "content") else { panic!("content array") };
        let Some(WireValue::String(text)) = field(&content[0], "text") else { panic!("text block") }; text
    }
    fn display_text(value: &WireValue) -> &WireString {
        let display = field(field(value, "details").unwrap(), "displayContent").unwrap();
        let Some(WireValue::String(text)) = field(display, "text") else { panic!("display text") }; text
    }
    async fn invoke(ctx: &ToolContext, path: &str, capture: Arc<Capture>) -> Result<ToolOutput, ara_agent::ToolError> {
        ReadTool { ctx: ctx.clone() }.execute_os_path_with_delimited_dispatcher(path, CancellationToken::new(), Some(capture)).await
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("nested")).unwrap();
    let cpu = cpu_fixture().to_string(); let sample = sample_fixture();
    let cpu_path = directory.path().join("nested").join("data.cpuprofile");
    let sample_path = directory.path().join("data.sample.txt");
    std::fs::write(&cpu_path, &cpu).unwrap(); std::fs::write(&sample_path, &sample).unwrap();
    let expected_cpu = render_cpu_profile(&cpu).unwrap().unwrap();
    let expected_sample = render_sample_profile(&sample).unwrap();
    let mut ctx = ToolContext::new(directory.path()).with_edit(ara_edit::EditMode::Replace, true)
        .with_read_render_markdown(true);
    for (path, expected) in [("data.cpuprofile", &expected_cpu), ("data.sample.txt", &expected_sample)] {
        let capture = Arc::new(Capture::default());
        let output = invoke(&ctx, path, capture.clone()).await.unwrap();
        let values = capture.outputs.lock().unwrap(); assert_eq!(values.len(), 1);
        assert_eq!(wire_text(&values[0]), expected);
        let details = field(&values[0], "details").unwrap();
        assert!(field(details, "suffixResolution").is_none(), "profile early return omits fuzzy suffix metadata");
        assert!(field(details, "contentType").is_none(), "profile summary never becomes Markdown contentType");
        assert_eq!(display_text(&values[0]), expected);
        assert_eq!(text(&output), expected.to_utf8().unwrap());
        assert!(output.details.as_ref().unwrap()["meta"]["source"]["type"] == "path");
    }
    // Internal skill files bypass the on-disk profiler gate even when their
    // physical name and JSON are eligible. Preserve internal source ownership.
    let skill_ctx = ctx.clone().with_skills(vec![ara_tools::internal_urls::SkillRef {
        name: "profile".into(), file_path: cpu_path.clone(), base_dir: directory.path().join("nested"),
    }]);
    let capture = Arc::new(Capture::default());
    let internal = invoke(&skill_ctx, "skill://profile", capture.clone()).await.unwrap();
    assert!(text(&internal).contains("callFrame"), "internal read remains original JSON");
    assert!(capture.outputs.lock().unwrap().is_empty());
    assert!(capture.errors.lock().unwrap().is_empty());
    assert_eq!(internal.details.as_ref().unwrap()["meta"]["source"]["type"], "internal");
    assert_eq!(internal.details.as_ref().unwrap()["meta"]["source"]["value"], "skill://profile");
    assert_eq!(internal.details.as_ref().unwrap()["resolvedPath"], json!(cpu_path.to_string_lossy()));
    assert_eq!(internal.details.as_ref().unwrap()["contentType"], "text/plain");
    let summary_lines = expected_cpu.to_utf8().unwrap().split_terminator('\n').map(str::to_owned).collect::<Vec<_>>();
    let capture = Arc::new(Capture::default());
    invoke(&ctx, "data.cpuprofile:2-2", capture.clone()).await.unwrap();
    let selected = capture.outputs.lock().unwrap().pop().unwrap();
    assert!(display_text(&selected).to_utf8().unwrap().contains(&summary_lines[1]));
    let capture = Arc::new(Capture::default());
    invoke(&ctx, "data.cpuprofile:1-1,3-3", capture.clone()).await.unwrap();
    let selected = capture.outputs.lock().unwrap().pop().unwrap();
    assert!(display_text(&selected).to_utf8().unwrap().contains(&summary_lines[0]));
    assert!(display_text(&selected).to_utf8().unwrap().contains(&summary_lines[2]));
    assert!(display_text(&selected).to_utf8().unwrap().contains('…'));
    let capture = Arc::new(Capture::default());
    let tail = invoke(&ctx, "data.cpuprofile:-2", capture.clone()).await.unwrap();
    assert!(text(&tail).contains(summary_lines.last().unwrap()));
    let capture = Arc::new(Capture::default());
    let beyond = invoke(&ctx, "data.cpuprofile:999999-999999", capture.clone()).await.unwrap();
    assert!(text(&beyond).contains("beyond end of profile summary"));
    let capture = Arc::new(Capture::default());
    let raw = invoke(&ctx, "data.cpuprofile:raw:1-1", capture.clone()).await.unwrap();
    assert!(capture.outputs.lock().unwrap().is_empty());
    assert!(text(&raw).contains("callFrame"), "raw selector addresses original JSON");
    std::fs::write(directory.path().join("wearing.cpuprofile"), "ordinary extension-only notes\n").unwrap();
    std::fs::write(directory.path().join("wearing.sample.txt"), "ordinary extension-only notes\n").unwrap();
    for path in ["wearing.cpuprofile", "wearing.sample.txt"] {
        let capture = Arc::new(Capture::default());
        assert!(text(&invoke(&ctx, path, capture.clone()).await.unwrap()).contains("ordinary extension-only notes"));
        assert!(capture.outputs.lock().unwrap().is_empty());
    }
    // File.text primitive: BOM stripped and CRLF retained, unlike pure input.
    let bom_path = directory.path().join("bom.sample.txt");
    let mut bytes = vec![0xef, 0xbb, 0xbf]; bytes.extend(sample.as_bytes()); std::fs::write(&bom_path, bytes).unwrap();
    let capture = Arc::new(Capture::default());
    invoke(&ctx, "bom.sample.txt", capture.clone()).await.unwrap();
    assert_eq!(wire_text(&capture.outputs.lock().unwrap()[0]), &expected_sample);
    std::fs::write(directory.path().join("crlf.sample.txt"), sample.replace('\n', "\r\n")).unwrap();
    let capture = Arc::new(Capture::default());
    let crlf = invoke(&ctx, "crlf.sample.txt", capture.clone()).await.unwrap();
    assert!(capture.outputs.lock().unwrap().is_empty(), "exact Call graph: does not match CRLF file text");
    assert!(text(&crlf).contains("Call graph:"));
    // Inclusive <=32MiB gate and the immediately larger bypass, in one fixture.
    let maximum = 32 * 1024 * 1024;
    let cap_path = directory.path().join("cap.cpuprofile");
    let mut padded = cpu.clone().into_bytes(); padded.resize(maximum, b' '); std::fs::write(&cap_path, &padded).unwrap();
    let capture = Arc::new(Capture::default()); invoke(&ctx, "cap.cpuprofile", capture.clone()).await.unwrap();
    assert_eq!(wire_text(&capture.outputs.lock().unwrap()[0]), &expected_cpu);
    padded.push(b' '); std::fs::write(&cap_path, &padded).unwrap();
    let capture = Arc::new(Capture::default());
    let oversized = invoke(&ctx, "cap.cpuprofile:1-1", capture.clone()).await.unwrap();
    assert!(capture.outputs.lock().unwrap().is_empty()); assert!(text(&oversized).contains("callFrame"));
    // Known image MIME wins despite a profiler extension; no renderer call.
    let png: &[u8] = &[137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82,0,0,0,1,0,0,0,1,8,6,0,0,0];
    std::fs::write(directory.path().join("image.cpuprofile"), png).unwrap();
    assert!(ara_tools::read_image::read_image_metadata(&directory.path().join("image.cpuprofile")).await.unwrap().is_some());
    let capture = Arc::new(Capture::default()); let _ = invoke(&ctx, "image.cpuprofile", capture.clone()).await;
    assert!(capture.outputs.lock().unwrap().is_empty());
    // One exact renderer: lone units are retained in both fields; only native
    // snapshot String conversion may replace them (fixed NAPI observation).
    let lone = cpu.replace("\"functionName\":\"", "\"functionName\":\"\\ud800");
    std::fs::write(directory.path().join("lone.cpuprofile"), &lone).unwrap();
    ctx = ctx.with_edit(ara_edit::EditMode::Hashline, true);
    let capture = Arc::new(Capture::default()); invoke(&ctx, "lone.cpuprofile", capture.clone()).await.unwrap();
    let exact = capture.outputs.lock().unwrap().pop().unwrap();
    assert!(wire_text(&exact).units().contains(&0xd800));
    assert!(display_text(&exact).units().contains(&0xd800));
    let key = ara_edit::path_policy::canonical_key(&directory.path().join("lone.cpuprofile"));
    let snapshot = ctx.edit_store.head(&key).unwrap(); assert!(snapshot.text.contains('\u{fffd}'));
    assert!(!snapshot.text.contains("callFrame"), "snapshot is rendered summary, not original JSON");
    let failure = r#"{"nodes":[{"id":1,"callFrame":{"functionName":"f"},"hitCount":1,"children":{}}],"startTime":0,"endTime":10}"#;
    std::fs::write(directory.path().join("failure.cpuprofile"), failure).unwrap();
    let capture = Arc::new(Capture::default()); assert!(invoke(&ctx, "failure.cpuprofile", capture.clone()).await.is_err());
    assert!(capture.outputs.lock().unwrap().is_empty()); assert_eq!(capture.errors.lock().unwrap().len(), 1);
}
