//! Replays independently executed, fixed-source Bun schema/validation/file cases.
use ara_cli::{
    model_config_file::{ConfigErrorStage, ModelConfigLoad, ModelsConfigFile},
    models_config::{ModelsConfig, ValidationMode, validate_provider_configuration},
};
use serde_json::Value;

#[test]
#[ignore = "requires a retained fixed-source Bun oracle in ARA_MODEL_CONFIG_ORACLE"]
fn fixed_source_models_configuration_comparison() {
    let path = std::env::var_os("ARA_MODEL_CONFIG_ORACLE").expect("execute scripts/model_config_oracle.py first");
    let oracle: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(oracle["upstreamCommit"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    assert_eq!(oracle["bunVersion"], "1.4.0");
    let cases = oracle["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 842, "complete generated case set required");
    for (family, mode, expected) in [
        ("schema", None, 485),
        ("provider", Some("models-config"), 173),
        ("provider", Some("runtime-register"), 173),
        ("file", None, 11),
    ] {
        let actual =
            cases.iter().filter(|case| case["kind"] == family && mode.is_none_or(|mode| case["mode"] == mode)).count();
        assert_eq!(actual, expected, "missing {family}/{mode:?} oracle coverage");
    }
    for (i, case) in cases.iter().enumerate() {
        let label = format!("case {i}: {}", case["label"]);
        match case["kind"].as_str().unwrap() {
            "schema" => {
                let actual = ModelsConfig::validate_schema(case["input"].clone());
                assert_eq!(actual.is_ok(), case["ok"].as_bool().unwrap(), "{label}, {actual:?}");
                if let Ok(actual) = actual {
                    assert_eq!(actual.value(), &case["value"], "{label}");
                }
            }
            "provider" => {
                let mode = match case["mode"].as_str() {
                    Some("models-config") => ValidationMode::ModelsConfig,
                    Some("runtime-register") => ValidationMode::RuntimeRegister,
                    other => panic!("unknown provider validation mode {other:?}"),
                };
                let actual = validate_provider_configuration("fixture", &case["input"], mode);
                assert_eq!(actual.is_ok(), case["ok"].as_bool().unwrap(), "{label}, {actual:?}");
                if let Err(actual) = actual {
                    assert_eq!(actual.to_string(), case["error"].as_str().unwrap(), "{label}");
                }
            }
            "file" => {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join(format!("models.{}", case["ext"].as_str().unwrap()));
                std::fs::write(&path, case["content"].as_str().unwrap()).unwrap();
                let actual = ModelsConfigFile::new(path).unwrap().try_load();
                match actual {
                    ModelConfigLoad::Ok(value) => {
                        assert_eq!(case["status"], "ok", "{label}");
                        assert_eq!(value.value(), &case["value"], "{label}");
                    }
                    ModelConfigLoad::NotFound => assert_eq!(case["status"], "not-found", "{label}"),
                    ModelConfigLoad::Error(error) => {
                        assert_eq!(case["status"], "error", "{label}, {error}");
                        let stage = match error.stage {
                            ConfigErrorStage::Schema => "Schema",
                            ConfigErrorStage::ValidateModels => "Validate(models)",
                            ConfigErrorStage::Unexpected => "Unexpected",
                            ConfigErrorStage::Read => "Read",
                            ConfigErrorStage::CreateDefault => "createDefault",
                        };
                        assert_eq!(case["stage"], stage, "{label}, {error}");
                    }
                }
            }
            _ => panic!("unexpected oracle family"),
        }
    }
    eprintln!("{} fixed-source schema/provider/native-file comparisons passed", cases.len());
}
