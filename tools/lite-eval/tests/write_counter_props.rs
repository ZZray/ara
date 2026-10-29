//! Property 27: write counts (requirement 20.4).
//!
//! Random sequences of `ws_write_file` and `ws_edit_file` calls (existing or
//! new targets, applicable or inapplicable edits, paths outside the
//! workspace) through the real wrappers over the Core builtins. The four
//! counters must equal a reference model of the same sequence.

use std::{collections::BTreeMap, sync::Arc};

use ara_agent_lite_harness::core::rpi_agent::ToolResultPartial;
use lite_eval::{
    host::{tools::ws_tool, WriteCounters, WriteCounts},
    workspace::canonical,
};
use proptest::prelude::*;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const FILES: [&str; 4] = ["a.txt", "src/b.cpp", "src/new/c.hpp", "d.md"];

#[derive(Clone, Debug)]
enum Op {
    Write { file: usize },
    Edit { file: usize, applicable: bool },
    WriteOutside,
    EditOutside,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..FILES.len()).prop_map(|file| Op::Write { file }),
        4 => (0..FILES.len(), any::<bool>()).prop_map(|(file, applicable)| Op::Edit { file, applicable }),
        1 => Just(Op::WriteOutside),
        1 => Just(Op::EditOutside),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn counters_match_the_reference_model(
        initial in proptest::collection::vec(any::<bool>(), FILES.len()),
        ops in proptest::collection::vec(op(), 0..12),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let base = canonical(dir.path()).unwrap();
        let root = base.join("ws");
        std::fs::create_dir_all(root.join("src")).unwrap();
        // Model: file → current content (present means it exists).
        let mut model: BTreeMap<usize, String> = BTreeMap::new();
        for (file, exists) in initial.iter().enumerate() {
            if *exists {
                let path = root.join(FILES[file]);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let content = format!("init-{file}\n");
                std::fs::write(&path, &content).unwrap();
                model.insert(file, content);
            }
        }
        let counters = Arc::new(WriteCounters::default());
        let write = ws_tool("ws_write_file", &root, counters.clone(), true).unwrap();
        let edit = ws_tool("ws_edit_file", &root, counters.clone(), true).unwrap();
        prop_assert!(ws_tool("ws_edit_file", &root, counters.clone(), false).is_none(), "v1 has no edit tool");
        let mut expected = WriteCounts::default();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let update: Arc<dyn Fn(ToolResultPartial) + Send + Sync> = Arc::new(|_| {});
        for (step, op) in ops.iter().enumerate() {
            let (tool, args) = match op {
                Op::Write { file } => {
                    let content = format!("write-{step}\n");
                    if model.insert(*file, content.clone()).is_some() {
                        expected.write_existing += 1;
                    } else {
                        expected.write_new += 1;
                    }
                    (&write, json!({"path": FILES[*file], "content": content}))
                }
                Op::Edit { file, applicable } => {
                    let current = model.get(file).cloned();
                    let old = match (&current, applicable) {
                        (Some(content), true) => content.clone(),
                        _ => format!("absent-{step}"),
                    };
                    let new = format!("edit-{step}\n");
                    if current.is_some() && *applicable {
                        expected.edit_ok += 1;
                        model.insert(*file, new.clone());
                    } else {
                        expected.edit_rejected += 1;
                    }
                    (&edit, json!({"path": FILES[*file], "old_text": old, "new_text": new}))
                }
                Op::WriteOutside => (&write, json!({"path": "../outside.txt", "content": "x"})),
                Op::EditOutside => {
                    expected.edit_rejected += 1;
                    (&edit, json!({"path": "../outside.txt", "old_text": "x", "new_text": "y"}))
                }
            };
            let result = runtime.block_on(tool.execute("call", args, CancellationToken::new(), update.clone()));
            if matches!(op, Op::WriteOutside | Op::EditOutside) {
                let error = result.err().map(|error| error.to_string()).unwrap_or_default();
                prop_assert!(error.contains("path_outside_workspace"), "{}", error);
            }
        }
        prop_assert!(!base.join("outside.txt").exists());
        prop_assert_eq!(counters.snapshot(), expected);
        for (file, content) in &model {
            prop_assert_eq!(&std::fs::read_to_string(root.join(FILES[*file])).unwrap(), content);
        }
    }
}
