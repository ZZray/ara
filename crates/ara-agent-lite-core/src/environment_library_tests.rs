use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("handwave-knowledge-{:016x}", u64::from_le_bytes(random)));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn roots(&self) -> Roots {
        resolve(&self.0.join("state"), &self.0, None).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn tool(roots: Roots, name: &str) -> KnowledgeTool {
    KnowledgeTool {
        roots,
        run: "run-test".into(),
        grant: "grant-secret".into(),
        evidence: Arc::new(|ids| {
            Ok(
                json!({"source":"native_current_run","run":"run-test","receipts":ids.iter().map(|id|json!({"tool_call_id":id,"tool_name":"exec","is_error":false,"output_hash":"0".repeat(64)})).collect::<Vec<_>>()}),
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
fn write_note(roots: &Roots, id: &str, expected: Option<&str>) -> Value {
    let mut args = json!({"scope":"project","id":id,"title":"Python environment","content":"Use the isolated Python interpreter. grant-secret"});
    if let Some(hash) = expected {
        args["expected_hash"] = json!(hash);
    }
    tool(roots.clone(), "knowledge_write").perform(args, &CancellationToken::new()).unwrap()
}

#[test]
fn resolves_closest_git_root_without_creating_and_respects_global_override() {
    let f = Fixture::new();
    let outer = f.0.join("repo");
    let inner = outer.join("nested");
    let cwd = inner.join("src");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir(outer.join(".git")).unwrap();
    fs::write(inner.join(".git"), "gitdir: somewhere").unwrap();
    let global = f.0.join("existing-global");
    let roots = resolve(&f.0.join("state"), &cwd, Some(&global)).unwrap();
    assert_eq!(roots.project_knowledge, inner.join("docs/knowledge"));
    assert_eq!(roots.global_knowledge, global);
    assert_eq!(roots.project_skills, inner.join(".handwave/skills"));
    assert!(!roots.environment_dir.exists());
    let missing = catalog(&roots).unwrap();
    assert_eq!(missing["indexes"][0]["status"], "missing");
    assert!(!roots.environment_dir.exists());
}

#[test]
fn canonicalized_runtime_cwd_initializes_catalogs_and_reuses_regular_notes() {
    let f = Fixture::new();
    let cwd = fs::canonicalize(&f.0).unwrap();
    let roots = resolve(&f.0.join("state"), &cwd, None).unwrap();
    initialize(&roots).unwrap();
    assert!(catalog(&roots).unwrap()["indexes"].as_array().unwrap().iter().all(|row| row["status"] == "available"));
    let note = write_note(&roots, "canonical-runtime", None);
    let path = note["entry"]["path"].as_str().unwrap();
    let reader = tool(roots.clone(), "knowledge_read");
    let response = reader.perform(json!({"scope":"project","path":path}), &CancellationToken::new()).unwrap();
    assert_eq!(response["sha256"], note["entry"]["sha256"]);
    assert_eq!(response["status"], "draft");
    let outside = f.0.join("outside.md");
    fs::write(&outside, "outside inode must not be read or changed").unwrap();
    fs::hard_link(&outside, roots.project_knowledge.join("linked-inode.md")).unwrap();
    assert!(reader.perform(json!({"scope":"project","path":"linked-inode.md"}), &CancellationToken::new()).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside inode must not be read or changed");
}

#[test]
fn initialization_preserves_existing_index_and_reuses_notes_across_instances() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let path = roots.project_knowledge.join("INDEX.md");
    fs::write(&path, "# Maintained by humans\n\n- [Guide](guide.md)\n").unwrap();
    fs::write(roots.project_knowledge.join("guide.md"), "Existing Python guide").unwrap();
    initialize(&roots).unwrap();
    assert!(read(&path, FILE_CAP).unwrap().starts_with("# Maintained by humans"));
    let result = write_note(&roots, "python", None);
    assert_eq!(result["entry"]["status"], "draft");
    let relative = result["entry"]["path"].as_str().unwrap();
    let text = read(&roots.project_knowledge.join(relative), FILE_CAP).unwrap();
    assert!(!text.contains("grant-secret"));
    assert!(text.contains("[redacted grant]"));
    let again = resolve(&f.0.join("state"), &f.0, None).unwrap();
    let hits = tool(again, "knowledge_search")
        .perform(json!({"scope":"project","query":"Python"}), &CancellationToken::new())
        .unwrap();
    assert!(hits["matches"].as_array().unwrap().iter().any(|v| v["path"] == "guide.md"));
    assert!(hits["matches"].as_array().unwrap().iter().any(|v| v["path"] == relative));
    assert!(!roots.project_skills.exists());
    assert!(!roots.global_skills.exists());
}

#[test]
fn observed_requires_current_run_success_and_revoke_preserves_content() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let writer = tool(roots.clone(), "knowledge_write");
    let result=writer.perform(json!({"scope":"global","id":"tool","title":"Tool installed","content":"Version verified","evidence_ids":["call-1"]}),&CancellationToken::new()).unwrap();
    assert_eq!(result["entry"]["status"], "observed");
    let hash = result["entry"]["sha256"].as_str().unwrap();
    let path = result["entry"]["path"].as_str().unwrap();
    let revoke = tool(roots.clone(), "knowledge_revoke")
        .perform(json!({"scope":"global","id":"tool","expected_hash":hash}), &CancellationToken::new())
        .unwrap();
    assert_eq!(revoke["entry"]["status"], "revoked");
    assert!(roots.global_knowledge.join(path).exists());
    let mut bad = writer;
    bad.evidence = Arc::new(|_| Ok(json!({"receipts":[{"is_error":true}]})));
    assert!(bad
        .perform(
            json!({"scope":"global","id":"failed","title":"bad","content":"claim","evidence_ids":["call-2"]}),
            &CancellationToken::new()
        )
        .is_err());
}

#[test]
fn conflict_damaged_manifest_and_cancel_leave_old_index_unchanged() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let result = write_note(&roots, "python", None);
    let indexpath = roots.project_knowledge.join("INDEX.md");
    let before = read(&indexpath, FILE_CAP).unwrap();
    let writer = tool(roots.clone(), "knowledge_write");
    let args = json!({"scope":"project","id":"python","title":"changed","content":"new"});
    assert!(writer.perform(args.clone(), &CancellationToken::new()).is_err());
    assert_eq!(read(&indexpath, FILE_CAP).unwrap(), before);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(writer.perform(json!({"scope":"project","id":"new-note","title":"new","content":"new"}), &cancel).is_err());
    assert_eq!(read(&indexpath, FILE_CAP).unwrap(), before);
    let revised = write_note(&roots, "python", result["entry"]["sha256"].as_str());
    assert!(roots.project_knowledge.join(revised["entry"]["path"].as_str().unwrap()).exists());
    assert!(roots.project_knowledge.join(result["entry"]["path"].as_str().unwrap()).exists());
    fs::write(&indexpath, format!("{BEGIN}\ndamaged\n{END}")).unwrap();
    let damaged = fs::read(&indexpath).unwrap();
    assert!(writer
        .perform(json!({"scope":"project","id":"new-note","title":"new","content":"new"}), &CancellationToken::new())
        .is_err());
    assert_eq!(fs::read(indexpath).unwrap(), damaged);
}

#[test]
fn index_overflow_fails_before_content_publication() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let indexpath = roots.project_knowledge.join("INDEX.md");
    let before = "x".repeat(FILE_CAP - 16);
    fs::write(&indexpath, &before).unwrap();
    let writer = tool(roots.clone(), "knowledge_write");
    assert!(writer
        .perform(json!({"scope":"project","id":"new-note","title":"new","content":"new"}), &CancellationToken::new())
        .is_err());
    assert_eq!(read(&indexpath, FILE_CAP).unwrap(), before);
    assert!(!roots.project_knowledge.join("notes").exists());
}

#[test]
fn shared_lock_prevents_lost_updates_and_read_paths_are_bounded() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let lock = open_file(&roots.project_knowledge.join(".handwave-library.lock"), true, true).unwrap();
    lock.try_lock_exclusive().unwrap();
    let writer = tool(roots.clone(), "knowledge_write");
    assert!(writer
        .perform(json!({"scope":"project","id":"new-note","title":"new","content":"new"}), &CancellationToken::new())
        .is_err());
    drop(lock);
    write_note(&roots, "one", None);
    let reader = tool(roots.clone(), "knowledge_read");
    for path in ["../secret.md", "C:/secret.md", "/etc/passwd.md", "notes\\secret.md", ".hidden.md", "INDEX.txt"] {
        assert!(reader.perform(json!({"scope":"project","path":path}), &CancellationToken::new()).is_err(), "{path}");
    }
    fs::write(roots.project_knowledge.join("huge.md"), "x".repeat(FILE_CAP + 1)).unwrap();
    assert!(reader.perform(json!({"scope":"project","path":"huge.md"}), &CancellationToken::new()).is_err());
    assert!(reader.perform(json!({"scope":"private","path":"INDEX.md"}), &CancellationToken::new()).is_err());
}

#[test]
fn catalog_truncates_independently_and_search_reports_partial() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    fs::write(roots.global_knowledge.join("INDEX.md"), "汉".repeat(4000)).unwrap();
    fs::write(roots.project_knowledge.join("INDEX.md"), "- [missing](missing.md)\n- [escape](../outside.md)\n")
        .unwrap();
    let value = catalog(&roots).unwrap();
    assert_eq!(value["indexes"][0]["truncated"], true);
    assert!(value["indexes"][0]["text"].as_str().unwrap().len() <= 8192);
    let result = tool(roots, "knowledge_search")
        .perform(json!({"scope":"project","query":"anything"}), &CancellationToken::new())
        .unwrap();
    assert_eq!(result["partial"], true);
    assert_eq!(result["unreadable"], 1);
}

#[test]
fn revoked_notes_are_not_reused_or_rediscovered_through_index_metadata() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let note = write_note(&roots, "python", None);
    let path = note["entry"]["path"].as_str().unwrap();
    tool(roots.clone(), "knowledge_revoke")
        .perform(
            json!({"scope":"project","id":"python","expected_hash":note["entry"]["sha256"]}),
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(tool(roots.clone(), "knowledge_read")
        .perform(json!({"scope":"project","path":path}), &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("revoked"));
    let search = tool(roots.clone(), "knowledge_search")
        .perform(json!({"scope":"project","query":"Python"}), &CancellationToken::new())
        .unwrap();
    assert!(search["matches"].as_array().unwrap().is_empty());
    assert!(roots.project_knowledge.join(path).exists());
}

#[test]
fn stale_writer_cannot_reactivate_revoked_id_but_new_observation_id_is_allowed() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let old = write_note(&roots, "python", None);
    let hash = old["entry"]["sha256"].as_str().unwrap();
    let revoke = tool(roots.clone(), "knowledge_revoke");
    let args = json!({"scope":"project","id":"python","expected_hash":hash});
    revoke.perform(args.clone(), &CancellationToken::new()).unwrap();
    // A repeated revoke is idempotent while the same immutable revision remains revoked.
    revoke.perform(args, &CancellationToken::new()).unwrap();
    let index = roots.project_knowledge.join("INDEX.md");
    let before = fs::read(&index).unwrap();
    let error = tool(roots.clone(), "knowledge_write")
        .perform(
            json!({"scope":"project","id":"python","title":"Stale writer","content":"Old run tries to reactivate","expected_hash":hash}),
            &CancellationToken::new(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("cannot be reactivated"));
    assert_eq!(fs::read(index).unwrap(), before);
    let new = write_note(&roots, "python-rechecked", None);
    assert_eq!(new["entry"]["status"], "draft");
}

#[test]
fn superseded_or_unpublished_revisions_are_audit_only_even_when_human_index_links_them() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let old = write_note(&roots, "python", None);
    let path = old["entry"]["path"].as_str().unwrap();
    let current = write_note(&roots, "python", old["entry"]["sha256"].as_str());
    let index = roots.project_knowledge.join("INDEX.md");
    let original = fs::read_to_string(&index).unwrap();
    fs::write(index, format!("- [historical source]({path})\n{original}")).unwrap();
    let reader = tool(roots.clone(), "knowledge_read");
    assert!(reader
        .perform(json!({"scope":"project","path":path}), &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("superseded"));
    let search = tool(roots.clone(), "knowledge_search")
        .perform(json!({"scope":"project","query":"isolated Python interpreter"}), &CancellationToken::new())
        .unwrap();
    let hits = search["matches"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["path"], current["entry"]["path"]);
    assert_eq!(search["partial"], true);
    assert!(search["warnings"][0]["warning"].as_str().unwrap().contains("superseded"));
}

#[test]
fn altered_managed_body_is_rejected_with_partial_search_warning_and_human_notes_are_unverified() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let note = write_note(&roots, "python", None);
    let path = note["entry"]["path"].as_str().unwrap();
    fs::write(roots.project_knowledge.join(path), "tampered Python source").unwrap();
    let reader = tool(roots.clone(), "knowledge_read");
    assert!(reader
        .perform(json!({"scope":"project","path":path}), &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("integrity_mismatch"));
    let search = tool(roots.clone(), "knowledge_search")
        .perform(json!({"scope":"project","query":"Python"}), &CancellationToken::new())
        .unwrap();
    assert!(search["matches"].as_array().unwrap().is_empty());
    assert_eq!(search["partial"], true);
    assert!(search["warnings"][0]["warning"].as_str().unwrap().contains("integrity_mismatch"));
    fs::write(roots.project_knowledge.join("human.md"), "Human-maintained knowledge").unwrap();
    let human = reader.perform(json!({"scope":"project","path":"human.md"}), &CancellationToken::new()).unwrap();
    assert_eq!(human["status"], "unmanaged_unverified");
}

#[cfg(unix)]
#[test]
fn refuses_symbolic_links_and_hardlinks() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let outside = f.0.join("outside.md");
    fs::write(&outside, "secret").unwrap();
    symlink(&outside, roots.project_knowledge.join("link.md")).unwrap();
    fs::hard_link(&outside, roots.project_knowledge.join("hard.md")).unwrap();
    let reader = tool(roots.clone(), "knowledge_read");
    for path in ["link.md", "hard.md"] {
        assert!(reader.perform(json!({"scope":"project","path":path}), &CancellationToken::new()).is_err());
    }
    let linked_root = f.0.join("linked");
    symlink(&roots.project_knowledge, &linked_root).unwrap();
    assert!(check_path(&linked_root.join("INDEX.md")).is_err());
}

#[cfg(windows)]
#[test]
fn failed_index_replacement_preserves_previous_index_and_unreferenced_revision() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let index = roots.project_knowledge.join("INDEX.md");
    let before = fs::read(&index).unwrap();
    let original_permissions = fs::metadata(&index).unwrap().permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&index, readonly).unwrap();
    let result = tool(roots.clone(), "knowledge_write").perform(
        json!({"scope":"project","id":"unpublished","title":"Note","content":"Validated technical observation"}),
        &CancellationToken::new(),
    );
    fs::set_permissions(&index, original_permissions).unwrap();
    assert!(result.unwrap_err().to_string().contains("old index preserved"));
    assert_eq!(fs::read(index).unwrap(), before);
    assert_eq!(fs::read_dir(roots.project_knowledge.join("notes")).unwrap().count(), 1);
    assert!(!fs::read_to_string(roots.project_knowledge.join("INDEX.md")).unwrap().contains("unpublished"));
}

#[test]
fn refuses_hardlinks() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let outside = f.0.join("outside.md");
    fs::write(&outside, "secret").unwrap();
    fs::hard_link(outside, roots.project_knowledge.join("hard.md")).unwrap();
    assert!(tool(roots, "knowledge_read")
        .perform(json!({"scope":"project","path":"hard.md"}), &CancellationToken::new())
        .is_err());
}

#[cfg(windows)]
#[test]
fn refuses_junction_traversal() {
    let f = Fixture::new();
    let roots = f.roots();
    initialize(&roots).unwrap();
    let link = roots.project_knowledge.join("linked");
    let target = f.0.join("outside");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("secret.md"), "private").unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        // cmd builtins interpret embedded forward slashes as switches.
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(target.to_string_lossy().replace('/', "\\"))
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "junction creation failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );
    let result = tool(roots, "knowledge_read")
        .perform(json!({"scope":"project","path":"linked/secret.md"}), &CancellationToken::new());
    assert!(result.is_err());
    fs::remove_dir(link).unwrap();
}
