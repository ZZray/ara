use super::*;

struct Fixture {
    root: PathBuf,
    roots: Roots,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("handwave-skills-test-{}", random_id().unwrap()));
        let roots = Roots {
            environment_dir: root.join("environment"),
            global_knowledge: root.join("global-knowledge"),
            project_knowledge: root.join("project-knowledge"),
            global_skills: root.join("global-skills"),
            project_skills: root.join("project-skills"),
        };
        Self { root, roots }
    }
    fn tool(&self, name: &str) -> SkillTool {
        SkillTool {
            roots: self.roots.clone(),
            run_id: "run-test".into(),
            grant: "private-grant-value".into(),
            evidence: Arc::new(|ids| {
                Ok(
                    json!({"source":"native_current_run","run":"run-test","receipts":ids.iter().map(|id|json!({"tool_call_id":id,"tool_name":"run_script","is_error":false,"output_hash":"a".repeat(64)})).collect::<Vec<_>>()}),
                )
            }),
            schema: Tool {
                name: name.into(),
                description: String::new(),
                parameters: Schema(json!({})),
                constrained_sampling: None,
            },
        }
    }
    fn create(&self) -> Value {
        self.tool("skill_create").apply(json!({"scope":"project","name":"hello-tool","description":"Repeatable fixture workflow","instructions":"Run scripts/hello.sh and inspect its output.","resources":{"scripts/hello.sh":"echo hello\n","LICENSE":"Fixture license"}}),&CancellationToken::new()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}

#[test]
fn skill_catalog_is_read_only_and_has_builtin_entries() {
    let f = Fixture::new();
    let result = catalog(&f.roots).unwrap();
    assert!(!f.root.exists());
    assert_eq!(result["skills"].as_array().unwrap().len(), 2);
    assert_eq!(result["skills"][0]["name"], "skill-creator");
    assert!(result.to_string().len() < 8192);
    assert!(result["skills"][0].get("content").is_none());
}

#[test]
fn skill_create_read_restart_and_hash_cover_all_files() {
    let f = Fixture::new();
    let created = f.create();
    assert_eq!(created["status"], "unverified");
    assert_eq!(Path::new(created["bundle_dir"].as_str().unwrap()), f.roots.project_skills.join("hello-tool"));
    assert!(f.roots.project_skills.join("hello-tool/LICENSE").is_file());
    let read = f
        .tool("skill_read")
        .apply(json!({"scope":"project","name":"hello-tool","path":"scripts/hello.sh"}), &CancellationToken::new())
        .unwrap();
    assert_eq!(read["content"], "echo hello\n");
    assert_eq!(read["bundle_dir"], created["bundle_dir"]);
    assert!(Path::new(read["bundle_dir"].as_str().unwrap()).is_absolute());
    assert!(Path::new(read["file_path"].as_str().unwrap()).is_file());
    assert_eq!(read["content_hash"], created["content_hash"]);
    let restart = catalog(&f.roots).unwrap();
    assert!(restart["skills"].as_array().unwrap().iter().any(|v| v["name"] == "hello-tool"));
    fs::write(f.roots.project_skills.join("hello-tool/LICENSE"), "changed license").unwrap();
    let changed =
        f.tool("skill_read").apply(json!({"scope":"project","name":"hello-tool"}), &CancellationToken::new()).unwrap();
    assert_ne!(changed["content_hash"], created["content_hash"]);
}

#[test]
fn skill_install_copies_license_binary_and_script_without_execution() {
    let f = Fixture::new();
    let source = f.root.join("source");
    create_dir(&source.join("scripts")).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: external-tool\ndescription: external workflow\n---\nDo the task.\n")
        .unwrap();
    fs::write(source.join("LICENSE"), "Preserve upstream notice").unwrap();
    fs::write(source.join("asset.bin"), [0, 255, 42]).unwrap();
    let marker = f.root.join("unexpected-execution");
    fs::write(source.join("scripts/setup.ps1"), format!("Set-Content '{}' executed", marker.display())).unwrap();
    let installed = f
        .tool("skill_install")
        .apply(json!({"scope":"global","source_dir":source}), &CancellationToken::new())
        .unwrap();
    assert_eq!(installed["status"], "unverified");
    assert_eq!(Path::new(installed["bundle_dir"].as_str().unwrap()), f.roots.global_skills.join("external-tool"));
    assert!(!marker.exists());
    assert_eq!(fs::read(f.roots.global_skills.join("external-tool/asset.bin")).unwrap(), [0, 255, 42]);
    assert_eq!(
        fs::read_to_string(f.roots.global_skills.join("external-tool/LICENSE")).unwrap(),
        "Preserve upstream notice"
    );
    assert!(f
        .tool("skill_install")
        .apply(json!({"scope":"global","source_dir":source}), &CancellationToken::new())
        .is_err());
}

#[test]
fn skill_verify_requires_current_native_exec_and_exact_hash() {
    let f = Fixture::new();
    let created = f.create();
    let args = json!({"scope":"project","name":"hello-tool","expected_hash":created["content_hash"],"evidence_ids":["call-real"]});
    let verified = f.tool("skill_verify").apply(args.clone(), &CancellationToken::new()).unwrap();
    assert_eq!(verified["status"], "execution_evidence_attached");
    assert!(verified["note"].as_str().unwrap().contains("does not automatically prove"));
    let read =
        f.tool("skill_read").apply(json!({"scope":"project","name":"hello-tool"}), &CancellationToken::new()).unwrap();
    assert_eq!(read["status"], "execution_evidence_attached");
    let mut tool = f.tool("skill_verify");
    tool.evidence = Arc::new(|_| {
        Ok(
            json!({"source":"native_current_run","run":"run-test","receipts":[{"tool_call_id":"call-real","tool_name":"download_file","is_error":false}]}),
        )
    });
    assert!(tool.apply(args.clone(), &CancellationToken::new()).is_err());
    tool.evidence = Arc::new(|_| bail!("unknown current Run evidence"));
    assert!(tool.apply(args.clone(), &CancellationToken::new()).is_err());
    fs::write(f.roots.project_skills.join("hello-tool/scripts/hello.sh"), "echo changed").unwrap();
    assert!(f.tool("skill_verify").apply(args, &CancellationToken::new()).is_err());
    let changed = catalog(&f.roots).unwrap();
    assert_eq!(
        changed["skills"].as_array().unwrap().iter().find(|v| v["name"] == "hello-tool").unwrap()["status"],
        "unverified"
    );
}

#[test]
fn skill_remove_preserves_owned_files_and_unknown_is_refused() {
    let f = Fixture::new();
    let created = f.create();
    let args = json!({"scope":"project","name":"hello-tool","expected_hash":created["content_hash"]});
    let retired = f.tool("skill_remove").apply(args, &CancellationToken::new()).unwrap();
    assert_eq!(retired["status"], "retired");
    assert!(!f.roots.project_skills.join("hello-tool").exists());
    assert!(Path::new(retired["preserved_at"].as_str().unwrap()).join("LICENSE").exists());
    assert!(!catalog(&f.roots).unwrap().to_string().contains("hello-tool"));
    f.create();
    fs::remove_file(f.roots.project_skills.join("hello-tool").join(META)).unwrap();
    assert!(f
        .tool("skill_remove")
        .apply(
            json!({"scope":"project","name":"hello-tool","expected_hash":created["content_hash"]}),
            &CancellationToken::new()
        )
        .is_err());
    assert!(f.roots.project_skills.join("hello-tool/LICENSE").exists());
}

#[test]
fn builtin_read_only_and_paging_utf8() {
    let f = Fixture::new();
    let read = f
        .tool("skill_read")
        .apply(json!({"scope":"builtin","name":"skill-creator","max_bytes":15}), &CancellationToken::new())
        .unwrap();
    assert_eq!(read["status"], "builtin");
    assert_eq!(read["bundle_dir"], Value::Null);
    assert_eq!(read["file_path"], Value::Null);
    assert_eq!(read["truncated"], true);
    assert!(!f.root.exists());
    for op in ["skill_create", "skill_install", "skill_verify", "skill_remove"] {
        assert!(f
            .tool(op)
            .apply(json!({"scope":"builtin","name":"skill-creator"}), &CancellationToken::new())
            .is_err());
    }
    let text = "你好世界";
    let first = page(text, "hash", "unverified", &json!({"offset":1,"max_bytes":4})).unwrap();
    assert_eq!(first["offset"], 0);
    assert_eq!(first["content"], "你");
    let tail = page(text, "hash", "unverified", &json!({"offset":999,"max_bytes":4})).unwrap();
    assert_eq!(tail["content"], "");
    assert_eq!(tail["truncated"], false);
}

#[test]
fn skill_rejects_escape_reserved_path_collision_and_oversize() {
    let f = Fixture::new();
    for path in [
        "../escape",
        "/root",
        "a/../../escape",
        "a\\..\\escape",
        "C:/escape",
        "a//b",
        "nul.txt",
        "scripts/com1.exe",
        "x.",
        META,
    ] {
        assert!(relative(path).is_err(), "{path}");
    }
    for name in ["..", "../a", "UPPER", "nul", "with space", "x/y", "a:"] {
        assert!(!valid_name(name));
    }
    let mut args = json!({"scope":"project","name":"test-tool","description":"test","instructions":"do task","resources":{"a":"a","A":"b"}});
    assert!(f.tool("skill_create").apply(args.clone(), &CancellationToken::new()).is_err());
    args["resources"] = json!({"huge":"x".repeat(FILE_LIMIT+1)});
    assert!(f.tool("skill_create").apply(args, &CancellationToken::new()).is_err());
    assert!(!f.root.join("escape").exists());
    assert!(!f.roots.project_skills.join("test-tool").exists());
}

#[test]
fn skill_grant_redacted_and_failure_cleans_only_own_stage() {
    let f = Fixture::new();
    let secret = "private-grant-value";
    let result=f.tool("skill_create").apply(json!({"scope":"project","name":"redacted-tool","description":format!("use {secret}"),"instructions":format!("never retain {secret}"),"resources":{"script.py":format!("token='{secret}'")}}),&CancellationToken::new()).unwrap();
    let files = scan(&f.roots.project_skills.join("redacted-tool"), &CancellationToken::new()).unwrap();
    assert!(!result.to_string().contains(secret));
    assert!(files.values().all(|b| !String::from_utf8_lossy(b).contains(secret)));
    let marker = f.roots.project_skills.join(".skill-stage-unknown");
    create_dir(&marker).unwrap();
    fs::write(marker.join("preserve"), "untouched").unwrap();
    assert!(f.tool("skill_create").apply(json!({"scope":"project","name":"broken-tool","description":"test","instructions":"test","resources":{"a":"file","a/b":"conflicting child"}}),&CancellationToken::new()).is_err());
    assert!(marker.join("preserve").exists());
    assert!(!f.roots.project_skills.join("broken-tool").exists());
    assert_eq!(
        fs::read_dir(&f.roots.project_skills)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".skill-stage-"))
            .count(),
        1
    );
}

#[test]
fn skill_cancellation_before_mutation_and_lock_contention() {
    let f = Fixture::new();
    let signal = CancellationToken::new();
    signal.cancel();
    assert!(f.tool("skill_create").apply(json!({"scope":"project","name":"test"}), &signal).is_err());
    assert!(!f.root.exists());
    let _guard = lock(&f.roots.project_skills).unwrap();
    assert!(f
        .tool("skill_create")
        .apply(
            json!({"scope":"project","name":"test","description":"test","instructions":"test"}),
            &CancellationToken::new()
        )
        .is_err());
    assert!(!f.roots.project_skills.join("test").exists());
}

#[test]
fn skill_install_rejects_link_and_excessive_tree() {
    let f = Fixture::new();
    assert!(f
        .tool("skill_install")
        .apply(json!({"scope":"project","source_dir":"relative-skill"}), &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("absolute path"));
    let source = f.root.join("source");
    create_dir(&source).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: tree-tool\ndescription: tree\n---\ntest\n").unwrap();
    for i in 0..COUNT_LIMIT {
        fs::write(source.join(format!("r{i}.txt")), "x").unwrap();
    }
    assert!(scan(&source, &CancellationToken::new()).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = f.root.join("link");
        symlink(&source, &link).unwrap();
        assert!(scan(&link, &CancellationToken::new()).is_err());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_dir;
        let link = f.root.join("link");
        if symlink_dir(&source, &link).is_ok() {
            assert!(scan(&link, &CancellationToken::new()).is_err());
            fs::remove_dir(link).unwrap();
        }
    }
}

#[test]
fn installed_frontmatter_validation_and_empty_instructions() {
    assert!(header(b"no frontmatter").is_err());
    assert!(header(b"---\nname: okay\ndescription: yes\n---\n").is_err());
    assert!(header(b"---\nname: okay\nname: again\ndescription: yes\n---\nrun\n").is_err());
    let parsed = header(b"---\nname: 'good-name'\ndescription: \"A useful thing\"\n---\nrun\n").unwrap();
    assert_eq!(parsed, ("good-name".into(), "A useful thing".into()));
}

#[test]
fn catalog_has_a_shared_aggregate_budget_across_bundles_and_scopes() {
    let f = Fixture::new();
    for (scope, name) in [("project", "big-one"), ("global", "big-two")] {
        let resources =
            serde_json::Map::from_iter((0..3).map(|i| (format!("asset{i}.txt"), json!("x".repeat(900_000)))));
        f.tool("skill_create").apply(json!({"scope":scope,"name":name,"description":"Large test bundle","instructions":"Use assets","resources":resources}),&CancellationToken::new()).unwrap();
    }
    let result = catalog(&f.roots).unwrap();
    assert_eq!(result["partial"], true);
    assert!(result["coverage"]["bytes_read"].as_u64().unwrap() <= TOTAL_LIMIT as u64);
    assert!(result["skills"].as_array().unwrap().len() < 4);
    assert!(result.to_string().len() < 8192);
    // Direct selected reads remain possible even when full discovery is partial.
    assert!(f
        .tool("skill_read")
        .apply(json!({"scope":"global","name":"big-two","max_bytes":64}), &CancellationToken::new())
        .is_ok());
}
