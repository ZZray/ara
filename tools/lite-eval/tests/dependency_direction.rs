// Feature: ara-lite-core-capability, Property 37: 依赖方向检查
//! Requirement 27: the three real Lite workspaces read in locked mode follow
//! the dependency direction; fixture packages with injected violations of
//! 27.1 and 27.2 are judged failing; and over generated graphs the checks
//! report exactly the reachable violations with real chains.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use lite_eval::deps::{
    check_core, check_harness, metadata, DepKind, Graph, Package, Source, Violation, PRODUCT_CRATES,
};
use proptest::prelude::*;

const CORE: &str = "crates/ara-agent-lite-core";
const HARNESS: &str = "crates/ara-agent-lite-harness";
const JSONL: &str = "crates/ara-agent-lite-jsonl";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn render(violations: &[Violation]) -> String {
    violations.iter().map(|violation| format!("\n  {violation}")).collect()
}

#[test]
fn real_lite_workspaces_follow_the_dependency_direction() {
    let root = repo_root();
    let mut graphs = BTreeMap::new();
    let mut failures = Vec::new();
    for workspace in [CORE, HARNESS, JSONL] {
        match metadata(&root.join(workspace).join("Cargo.toml")) {
            Ok(graph) => {
                graphs.insert(workspace, graph);
            }
            Err(error) => failures.push(format!("{workspace}: {error:#}")),
        }
    }
    // 27.5: a workspace whose metadata fails is a failure, never skipped.
    assert!(failures.is_empty(), "cargo metadata failed:\n{}", failures.join("\n"));

    let core_root = root.join(CORE);
    let core_name = graphs[CORE].root_name().to_owned();
    let mut forbidden = vec![graphs[HARNESS].root_name(), graphs[JSONL].root_name()];
    forbidden.extend_from_slice(PRODUCT_CRATES);
    assert_eq!(forbidden[..2], ["ara-agent-lite-harness", "ara-agent-lite-jsonl"]);

    let mut violations = check_core(&graphs[CORE], &core_root, &forbidden);
    for workspace in [HARNESS, JSONL] {
        let graph = &graphs[workspace];
        violations.extend(check_harness(graph, &core_name, &core_root));
        // The Core node as resolved by this workspace's lock obeys 27.1 too.
        for (id, package) in &graph.packages {
            if package.name == core_name {
                violations.extend(check_core(&graph.rooted_at(id).unwrap(), &core_root, &forbidden));
            }
        }
    }
    assert!(violations.is_empty(), "dependency direction violations:{}", render(&violations));
}

/// Write a library package; `deps` are complete TOML lines per section.
fn package(dir: &Path, name: &str, version: &str, workspace: bool, sections: &[(&str, &[&str])]) {
    fs::create_dir_all(dir.join("src")).unwrap();
    let mut manifest = format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n");
    if workspace {
        manifest.push_str("[workspace]\n");
    }
    for (section, lines) in sections {
        manifest.push_str(&format!("[{section}]\n"));
        for line in *lines {
            manifest.push_str(line);
            manifest.push('\n');
        }
    }
    fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    fs::write(dir.join("src/lib.rs"), "").unwrap();
}

fn lock(dir: &Path) {
    let status = Command::new("cargo")
        .args(["generate-lockfile", "--offline", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .status()
        .unwrap();
    assert!(status.success(), "cannot lock fixture {}", dir.display());
}

fn last_names(graph: &Graph, violations: &[Violation]) -> BTreeSet<(String, String)> {
    violations
        .iter()
        .map(|violation| match violation {
            Violation::ForbiddenPackage { package, chain, .. } => {
                ("forbidden".to_owned(), format!("{package} via {}", chain_names(graph, chain)))
            }
            Violation::OutsidePathDependency { package, chain, .. } => {
                ("outside".to_owned(), format!("{package} via {}", chain_names(graph, chain)))
            }
            Violation::GitBranchDependency { package, chain, .. } => {
                ("branch".to_owned(), format!("{package} via {}", chain_names(graph, chain)))
            }
            Violation::CoreInstances { count, .. } => ("instances".to_owned(), count.to_string()),
            Violation::CoreNotLocalPath { chain, .. } => ("not-local".to_owned(), chain_names(graph, chain)),
        })
        .collect()
}

fn chain_names(graph: &Graph, chain: &[String]) -> String {
    chain.iter().map(|id| graph.packages[id].name.as_str()).collect::<Vec<_>>().join(">")
}

#[test]
fn injected_core_violations_of_27_1_fail() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    package(
        &base.join("harness"),
        "ara-agent-lite-harness",
        "0.1.0",
        false,
        &[("dependencies", &["lumen-core = { path = \"../lumen-core\" }"])],
    );
    package(&base.join("lumen-core"), "lumen-core", "0.1.0", false, &[]);
    package(&base.join("outside"), "outside-helper", "0.1.0", false, &[]);
    package(&base.join("handwave-ui"), "handwave-ui", "0.1.0", false, &[]);
    let core = base.join("core");
    package(&core.join("inner"), "core-inner", "0.1.0", false, &[]);
    package(
        &core,
        "fixture-core",
        "0.1.1",
        true,
        &[
            (
                "dependencies",
                &["ara-agent-lite-harness = { path = \"../harness\" }", "core-inner = { path = \"inner\" }"],
            ),
            ("build-dependencies", &["outside-helper = { path = \"../outside\" }"]),
            ("dev-dependencies", &["handwave-ui = { path = \"../handwave-ui\" }"]),
        ],
    );
    lock(&core);
    let graph = metadata(&core.join("Cargo.toml")).unwrap();
    let mut forbidden = vec!["ara-agent-lite-harness", "ara-agent-lite-jsonl"];
    forbidden.extend_from_slice(PRODUCT_CRATES);
    let violations = check_core(&graph, &core, &forbidden);
    let expected: BTreeSet<(String, String)> = [
        ("forbidden", "ara-agent-lite-harness via fixture-core>ara-agent-lite-harness"),
        ("outside", "ara-agent-lite-harness via fixture-core>ara-agent-lite-harness"),
        ("forbidden", "lumen-core via fixture-core>ara-agent-lite-harness>lumen-core"),
        ("outside", "lumen-core via fixture-core>ara-agent-lite-harness>lumen-core"),
        ("outside", "outside-helper via fixture-core>outside-helper"),
        ("forbidden", "handwave-ui via fixture-core>handwave-ui"),
        ("outside", "handwave-ui via fixture-core>handwave-ui"),
    ]
    .into_iter()
    .map(|(kind, text)| (kind.to_owned(), text.to_owned()))
    .collect();
    assert_eq!(last_names(&graph, &violations), expected, "{}", render(&violations));
    assert!(violations.iter().all(|violation| violation.to_string().contains(&graph.workspace)));
}

#[test]
fn injected_harness_violations_of_27_2_fail() {
    let root = repo_root();
    let real_core = root.join(CORE);
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    package(&base.join("core_a"), "ara-agent-lite-core", "0.1.1", false, &[]);
    package(&base.join("core_b"), "ara-agent-lite-core", "0.2.0", false, &[]);
    // Two Core instances, neither from this repository's path.
    let two = base.join("two");
    package(
        &two,
        "fixture-harness",
        "0.1.0",
        true,
        &[(
            "dependencies",
            &[
                "core_a = { package = \"ara-agent-lite-core\", path = \"../core_a\" }",
                "core_b = { package = \"ara-agent-lite-core\", path = \"../core_b\" }",
            ],
        )],
    );
    lock(&two);
    let graph = metadata(&two.join("Cargo.toml")).unwrap();
    let violations = check_harness(&graph, "ara-agent-lite-core", &real_core);
    let found = last_names(&graph, &violations);
    assert!(found.contains(&("instances".into(), "2".into())), "{}", render(&violations));
    assert_eq!(found.iter().filter(|(kind, _)| kind == "not-local").count(), 1, "same names collapse: {found:?}");
    assert_eq!(violations.len(), 3, "{}", render(&violations));

    // One instance, but a copy outside the repository (stands for git or crates.io).
    let copy = base.join("copy");
    package(
        &copy,
        "fixture-harness",
        "0.1.0",
        true,
        &[("dependencies", &["ara-agent-lite-core = { path = \"../core_a\" }"])],
    );
    lock(&copy);
    let graph = metadata(&copy.join("Cargo.toml")).unwrap();
    let violations = check_harness(&graph, "ara-agent-lite-core", &real_core);
    assert!(
        matches!(violations.as_slice(), [Violation::CoreNotLocalPath { chain, .. }] if chain.len() == 2),
        "{}",
        render(&violations)
    );
}

#[test]
fn a_workspace_whose_lock_disagrees_with_its_manifest_fails_and_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let stale = dir.path().join("stale-workspace");
    package(&stale.join("dep"), "fixture-dep", "0.1.0", false, &[]);
    package(&stale, "fixture-stale", "0.1.0", true, &[]);
    lock(&stale);
    // A dependency added after the lock was written: --locked must refuse.
    package(&stale, "fixture-stale", "0.1.0", true, &[("dependencies", &["fixture-dep = { path = \"dep\" }"])]);
    let error = format!("{:#}", metadata(&stale.join("Cargo.toml")).unwrap_err());
    assert!(error.contains("stale-workspace"), "{error}");
}

// ---- Property 37 over generated graphs ----

const CORE_NAME: &str = "ara-agent-lite-core";

#[derive(Clone, Debug)]
enum Origin {
    Registry,
    GitPinned,
    GitBranch,
    PathInside,
    PathOutside,
    PathCoreRoot,
}

fn origin() -> impl Strategy<Value = Origin> {
    prop_oneof![
        Just(Origin::Registry),
        Just(Origin::GitPinned),
        Just(Origin::GitBranch),
        Just(Origin::PathInside),
        Just(Origin::PathOutside),
        Just(Origin::PathCoreRoot),
    ]
}

fn kind() -> impl Strategy<Value = DepKind> {
    prop_oneof![Just(DepKind::Normal), Just(DepKind::Build), Just(DepKind::Dev)]
}

const NAMES: &[&str] = &[
    "serde",
    "tokio",
    "handwave-ui",
    "lantern",
    "paseo-x",
    "lumen",
    "ara-agent",
    "ara-workflow",
    "ara-agent-lite-harness",
    "ara-agent-lite-jsonl",
    "ara-agentx",
    CORE_NAME,
    CORE_NAME,
];

fn core_root() -> PathBuf {
    PathBuf::from("/fixture/core")
}

fn graph() -> impl Strategy<Value = Graph> {
    (1usize..8)
        .prop_flat_map(|count| {
            (
                prop::collection::vec((0..NAMES.len(), origin()), count),
                prop::collection::vec((0..=count, 1..=count, kind()), 0..count * 3),
            )
        })
        .prop_map(|(nodes, raw_edges)| {
            let id = |index: usize| if index == 0 { "root".to_owned() } else { format!("pkg{index}") };
            let mut packages = BTreeMap::from([(
                "root".to_owned(),
                Package { id: "root".into(), name: "root-package".into(), source: Source::Path(core_root()) },
            )]);
            for (index, (name, origin)) in nodes.iter().enumerate() {
                let index = index + 1;
                let source = match origin {
                    Origin::Registry => Source::Registry,
                    Origin::GitPinned => Source::Git { url: "https://x/y.git".into(), rev: Some("8ec4626".into()) },
                    Origin::GitBranch => Source::Git { url: "https://x/y.git".into(), rev: None },
                    Origin::PathInside => Source::Path(core_root().join(format!("sub{index}"))),
                    // A sibling whose name shares the textual prefix is still outside.
                    Origin::PathOutside => Source::Path(PathBuf::from(format!("/fixture/core-sibling{index}"))),
                    Origin::PathCoreRoot => Source::Path(core_root()),
                };
                packages.insert(id(index), Package { id: id(index), name: NAMES[*name].to_owned(), source });
            }
            let mut edges: BTreeMap<String, Vec<(String, DepKind)>> = BTreeMap::new();
            for (from, to, kind) in raw_edges {
                if from != to {
                    edges.entry(id(from)).or_default().push((id(to), kind));
                }
            }
            Graph { workspace: "fixture".into(), root: "root".into(), packages, edges }
        })
}

/// Reachable IDs by fixpoint, independent of the checker's breadth-first walk.
fn reachable(graph: &Graph) -> BTreeSet<String> {
    let mut seen = BTreeSet::from([graph.root.clone()]);
    loop {
        let next: BTreeSet<String> = seen
            .iter()
            .flat_map(|id| graph.edges.get(id).into_iter().flatten().map(|(dep, _)| dep.clone()))
            .chain(seen.iter().cloned())
            .collect();
        if next.len() == seen.len() {
            return seen;
        }
        seen = next;
    }
}

fn valid_chain(graph: &Graph, chain: &[String]) -> bool {
    chain.first() == Some(&graph.root)
        && chain
            .windows(2)
            .all(|pair| graph.edges.get(&pair[0]).is_some_and(|deps| deps.iter().any(|(dep, _)| *dep == pair[1])))
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) })]

    #[test]
    fn checks_report_exactly_reachable_violations_with_real_chains(graph in graph()) {
        let mut forbidden = vec!["ara-agent-lite-harness", "ara-agent-lite-jsonl"];
        forbidden.extend_from_slice(PRODUCT_CRATES);
        let reached = reachable(&graph);

        let mut expected = BTreeSet::new();
        for id in reached.iter().filter(|id| **id != graph.root) {
            let package = &graph.packages[id];
            let name = package.name.as_str();
            let product = PRODUCT_CRATES.iter().any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => name.starts_with(prefix),
                None => name == *pattern,
            });
            if product || name == "ara-agent-lite-harness" || name == "ara-agent-lite-jsonl" {
                expected.insert(("forbidden", id.clone()));
            }
            match &package.source {
                Source::Path(path) if !path.starts_with(core_root()) => {
                    expected.insert(("outside", id.clone()));
                }
                Source::Git { rev: None, .. } => {
                    expected.insert(("branch", id.clone()));
                }
                _ => {}
            }
        }
        let mut actual = BTreeSet::new();
        for violation in check_core(&graph, &core_root(), &forbidden) {
            let (tag, package, chain) = match violation {
                Violation::ForbiddenPackage { package, chain, .. } => ("forbidden", package, chain),
                Violation::OutsidePathDependency { package, chain, .. } => ("outside", package, chain),
                Violation::GitBranchDependency { package, chain, .. } => ("branch", package, chain),
                other => panic!("check_core reported {other:?}"),
            };
            let last = chain.last().unwrap().clone();
            prop_assert!(valid_chain(&graph, &chain), "chain {:?} is not a path of the graph", chain);
            prop_assert_eq!(&graph.packages[&last].name, &package);
            prop_assert!(actual.insert((tag, last)), "duplicate report");
        }
        prop_assert_eq!(&actual, &expected);

        let cores: Vec<&String> =
            reached.iter().filter(|id| graph.packages[*id].name == CORE_NAME).collect();
        let not_local: BTreeSet<String> = cores
            .iter()
            .filter(|id| graph.packages[**id].source != Source::Path(core_root()))
            .map(|id| (*id).clone())
            .collect();
        let mut instances = None;
        let mut reported_not_local = BTreeSet::new();
        for violation in check_harness(&graph, CORE_NAME, &core_root()) {
            match violation {
                Violation::CoreInstances { count, .. } => {
                    prop_assert!(instances.replace(count).is_none());
                }
                Violation::CoreNotLocalPath { chain, .. } => {
                    prop_assert!(valid_chain(&graph, &chain));
                    reported_not_local.insert(chain.last().unwrap().clone());
                }
                other => panic!("check_harness reported {other:?}"),
            }
        }
        prop_assert_eq!(instances, (cores.len() != 1).then_some(cores.len()));
        prop_assert_eq!(reported_not_local, not_local);
    }
}
