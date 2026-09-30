use ara_context::build_skill_prompt;
use ara_discovery::{Level, LoadedSkill, SourceMeta, parse_skill_invocation};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn skill(path: &Path, name: &str) -> LoadedSkill {
    LoadedSkill {
        name: name.into(),
        description: "fixture".into(),
        file_path: path.into(),
        base_dir: path.parent().unwrap().into(),
        source: "test:project".into(),
        hide: false,
        meta: SourceMeta::new("test", path, Level::Project),
    }
}

#[test]
fn fresh_body_uses_invocation_rules_not_discovery_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SKILL.md");
    let loaded = skill(&path, "reviewer");
    fs::write(&path, "---\nname: reviewer\n---\nOld body.\n").unwrap();
    let old = build_skill_prompt(&loaded, "").unwrap();
    assert!(old.message.contains("Old body."));
    assert_eq!(old.details["lineCount"], 1);
    assert!(old.details.get("args").is_none());
    fs::write(&path, "---\r\nname: reviewer\r\n---\r\n<!-- retained -->\r\nNew body.\r\n").unwrap();
    let new = build_skill_prompt(&loaded, "\u{a0}focus\u{a0}").unwrap();
    assert!(new.message.contains("name: reviewer"), "CRLF frontmatter is retained");
    assert!(new.message.contains("<!-- retained -->"), "body comments are not discovery comments");
    assert!(new.message.contains("New body.") && !new.message.contains("Old body."));
    assert_eq!(new.details["lineCount"], 5);
    assert_eq!(new.details["args"], "focus");
    fs::remove_file(&path).unwrap();
    assert_eq!(build_skill_prompt(&loaded, "").unwrap_err().kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn compares_with_executed_fixed_omp_invocation_oracle() {
    let Some(path) = std::env::var_os("ARA_CTX_INVOCATION_ORACLE") else {
        eprintln!("NOT RUN: set ARA_CTX_INVOCATION_ORACLE to the executed fixed-source oracle.json");
        return;
    };
    let path = std::path::PathBuf::from(path);
    let oracle: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(oracle["upstreamCommit"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    assert_eq!(oracle["bunVersion"], "1.4.0");
    assert_eq!(oracle["mockCalls"], 0);
    let template = path.parent().unwrap().join("upstream/packages/coding-agent/src/prompts/skills/user-invocation.md");
    assert_eq!(fs::read(template).unwrap(), include_bytes!("../prompts/skills/user-invocation.md"));
    let cases = oracle["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 35, "the executed case set must not silently shrink");
    for case in cases {
        let actual = match case["kind"].as_str().unwrap() {
            "parse" => parse_skill_invocation(case["input"].as_str().unwrap())
                .map(|parsed| json!({"name":parsed.name,"args":parsed.args}))
                .unwrap_or(Value::Null),
            "build" => {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("SKILL.md");
                let mut loaded = skill(&path, case["skill"]["name"].as_str().unwrap());
                loaded.base_dir = Path::new(case["skill"]["baseDir"].as_str().unwrap()).into();
                let mut expected = case["result"].clone();
                if expected["ok"] == true {
                    expected["details"]["path"] = json!(path.to_string_lossy());
                }
                if let Some(prior) = case["priorContentBytesBase64"].as_str() {
                    fs::write(&path, STANDARD.decode(prior).unwrap()).unwrap();
                    let prior = build_skill_prompt(&loaded, case["args"].as_str().unwrap()).unwrap();
                    let mut expected_prior = case["priorResult"].clone();
                    expected_prior["details"]["path"] = json!(path.to_string_lossy());
                    assert_eq!(
                        json!({"ok":true,"message":prior.message,"details":prior.details}),
                        expected_prior,
                        "{} prior read",
                        case["id"]
                    );
                }
                if let Some(bytes) = case["contentBytesBase64"].as_str() {
                    fs::write(&path, STANDARD.decode(bytes).unwrap()).unwrap();
                }
                match build_skill_prompt(&loaded, case["args"].as_str().unwrap()) {
                    Ok(built) => {
                        let actual = json!({"ok":true,"message":built.message,"details":built.details});
                        assert_eq!(actual, expected, "{}", case["id"]);
                        continue;
                    }
                    Err(error) => {
                        assert_eq!(case["id"], "missing-file");
                        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
                        assert_eq!(case["result"]["ok"], false);
                        continue;
                    }
                }
            }
            other => panic!("unsupported oracle case {other}"),
        };
        assert_eq!(actual, case["result"], "{}", case["id"]);
    }
}
