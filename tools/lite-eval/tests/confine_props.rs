// Feature: ara-lite-core-capability, Property 4: 工作区路径检查
//! `confine` accepts a path iff a reference resolver over a model of the
//! fixture tree (lexical `.`/`..`, then junctions followed) lands inside the
//! workspace root, with an existing final link or hard-linked file refused
//! for every intent except the search root. Rejections carry the known code
//! `path_outside_workspace` and never content of a file outside.

mod support;

use std::{fs, path::Path};

use lite_eval::{
    host::tools::{confine, Intent, PathRejection},
    workspace::canonical,
};
use proptest::{
    prelude::*,
    test_runner::{Config, TestRunner},
};

const OUTSIDE_MARKER: &str = "OUTSIDE-FILE-CONTENT";
const SEGMENTS: [&str; 12] =
    [".", "..", "ws", "src", "a.txt", "b.cpp", "jin", "jout", "outside", "secret.txt", "hl.txt", "new"];
/// Absolute starts that are never under the test base directory.
const FOREIGN: [&str; 3] = [r"C:\Windows", "/etc", r"D:\WorkSpace"];

#[derive(Clone, Copy, Debug)]
enum Start {
    /// Relative to the workspace root.
    Relative,
    /// Absolute path of the workspace root.
    AbsoluteRoot,
    /// Absolute path of the directory holding `ws` and `outside`.
    AbsoluteBase,
    Foreign(usize),
}

/// Node of the modelled tree, by its real (link-free) path below the base.
enum Node {
    Dir,
    File { links: u32 },
    Link(&'static [&'static str]),
}

fn lookup(dir: &[&str], name: &str) -> Option<Node> {
    let node = match (dir, name) {
        ([], "ws") | ([], "outside") | (["ws"], "src") => Node::Dir,
        (["ws"], "a.txt") | (["ws", "src"], "b.cpp") | (["outside"], "secret.txt") => Node::File { links: 1 },
        (["ws"], "hl.txt") | (["outside"], "hl.txt") => Node::File { links: 2 },
        (["ws"], "jin") => Node::Link(&["ws", "src"]),
        (["ws"], "jout") => Node::Link(&["outside"]),
        _ => return None,
    };
    Some(node)
}

/// Reference decision. `None` when the raw path is empty.
fn expected(start: Start, segments: &[&'static str], intent: Intent) -> Option<bool> {
    let mut lexical: Vec<&str> = match start {
        Start::Relative if segments.is_empty() => return None,
        Start::Relative | Start::AbsoluteRoot => vec!["ws"],
        Start::AbsoluteBase => vec![],
        Start::Foreign(_) => return Some(false),
    };
    for segment in segments {
        match *segment {
            "." => {}
            ".." => {
                // Above the base nothing leads back into the workspace.
                if lexical.pop().is_none() {
                    return Some(false);
                }
            }
            name => lexical.push(name),
        }
    }
    let mut real: Vec<&str> = vec![];
    let mut file_links = None;
    let mut final_link = false;
    for (index, name) in lexical.iter().enumerate() {
        if file_links.is_some() {
            // A name below a file does not exist; the file is the deepest
            // existing ancestor.
            return Some(real.first() == Some(&"ws"));
        }
        match lookup(&real, name) {
            None => return Some(real.first() == Some(&"ws")),
            Some(Node::Dir) => real.push(name),
            Some(Node::File { links }) => {
                real.push(name);
                file_links = Some(links);
            }
            Some(Node::Link(target)) => {
                real = target.to_vec();
                final_link = index + 1 == lexical.len();
            }
        }
    }
    let inside = real.first() == Some(&"ws");
    let plain = !final_link && file_links.is_none_or(|links| links == 1);
    Some(inside && (intent == Intent::SearchRoot || plain))
}

fn build(base: &Path) {
    let ws = base.join("ws");
    let outside = base.join("outside");
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(ws.join("a.txt"), b"inside a\n").unwrap();
    fs::write(ws.join("src").join("b.cpp"), b"int b;\n").unwrap();
    fs::write(outside.join("secret.txt"), OUTSIDE_MARKER).unwrap();
    fs::write(outside.join("hl.txt"), OUTSIDE_MARKER).unwrap();
    fs::hard_link(outside.join("hl.txt"), ws.join("hl.txt")).unwrap();
    support::dir_link(&ws.join("jin"), &ws.join("src")).unwrap();
    support::dir_link(&ws.join("jout"), &outside).unwrap();
}

fn intent() -> impl Strategy<Value = Intent> {
    prop_oneof![
        Just(Intent::ReadExisting),
        Just(Intent::WriteMaybeNew),
        Just(Intent::EditExisting),
        Just(Intent::SearchRoot)
    ]
}

fn start() -> impl Strategy<Value = Start> {
    prop_oneof![
        3 => Just(Start::Relative),
        2 => Just(Start::AbsoluteRoot),
        2 => Just(Start::AbsoluteBase),
        1 => (0..FOREIGN.len()).prop_map(Start::Foreign),
    ]
}

#[test]
fn confine_accepts_exactly_the_paths_that_resolve_inside() {
    let dir = tempfile::tempdir().unwrap();
    let base = canonical(dir.path()).unwrap();
    build(&base);
    let root = canonical(&base.join("ws")).unwrap();
    let strategy = (
        start(),
        prop::collection::vec(prop::sample::select(SEGMENTS.to_vec()), 0..6),
        prop::bool::ANY,
        prop::bool::ANY,
        intent(),
    );
    let mut runner = TestRunner::new(Config { failure_persistence: None, ..Config::with_cases(256) });
    runner
        .run(&strategy, |(start, segments, backslash, upper, intent)| {
            let separator = if backslash { "\\" } else { "/" };
            let prefix = match start {
                Start::Relative => String::new(),
                Start::AbsoluteRoot => root.to_string_lossy().into_owned(),
                Start::AbsoluteBase => base.to_string_lossy().into_owned(),
                Start::Foreign(index) => FOREIGN[index].to_owned(),
            };
            // NTFS names are case-insensitive; the canonical comparison must be too.
            let prefix = if upper && cfg!(windows) { prefix.to_uppercase() } else { prefix };
            let joined = segments.join(separator);
            let raw = match (prefix.is_empty(), joined.is_empty()) {
                (true, _) => joined,
                (false, true) => prefix,
                (false, false) => format!("{prefix}{separator}{joined}"),
            };
            let actual = confine(&root, &raw, intent);
            match expected(start, &segments, intent) {
                None => prop_assert_eq!(actual, Err(PathRejection::Empty)),
                Some(accept) => {
                    prop_assert_eq!(actual.is_ok(), accept, "raw {:?} intent {:?}: {:?}", raw, intent, actual);
                    match actual {
                        Ok(path) => {
                            prop_assert!(path.starts_with(&root), "{} escapes {}", path.display(), root.display());
                            if intent == Intent::ReadExisting && path.is_file() {
                                let bytes = fs::read(&path).unwrap();
                                prop_assert!(!String::from_utf8_lossy(&bytes).contains(OUTSIDE_MARKER));
                            }
                        }
                        Err(rejection) => {
                            prop_assert_eq!(rejection.code(), "path_outside_workspace");
                            let report = format!("{rejection:?}");
                            prop_assert!(!report.contains(OUTSIDE_MARKER));
                        }
                    }
                }
            }
            Ok(())
        })
        .unwrap();
}
