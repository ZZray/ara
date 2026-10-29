//! One grouped source-backed Native mode-resolution family; no environment I/O.
#[path = "../src/edit_mode_policy.rs"]
mod policy;

use ara_edit::EditMode;
use policy::{EditModeEnvironment, EditModeSettings, ResolvedEditMode, resolve_edit_mode};
use serde_json::json;

fn pair(tool_mode: EditMode, display_mode: EditMode) -> ResolvedEditMode {
    ResolvedEditMode { tool_mode, display_mode }
}

#[test]
fn native_precedence_adaptive_exceptions_and_captured_environment_family() {
    let model = json!({"provider":"custom", "id":"wire-model", "catalogModelId":"deepseek-chat"});
    let mut settings = EditModeSettings::default();
    let empty = EditModeEnvironment::default();
    assert_eq!(
        resolve_edit_mode(None, &settings, empty, Some(&model), true).unwrap(),
        pair(EditMode::Replace, EditMode::Replace)
    );
    assert_eq!(
        resolve_edit_mode(None, &settings, empty, None, true).unwrap(),
        pair(EditMode::Hashline, EditMode::Hashline)
    );
    settings.mode = Some(json!("hashline"));
    for strict in ["1", "Y", "y", "TRUE", "true", "YES", "yes", "ON", "on"] {
        let env = EditModeEnvironment { variant: None, strict: Some(strict) };
        assert_eq!(
            resolve_edit_mode(None, &settings, env, Some(&model), true).unwrap(),
            pair(EditMode::Hashline, EditMode::Hashline),
            "{strict}"
        );
    }
    for strict in ["", "0", "True", "Yes", "On", "false", " true "] {
        let env = EditModeEnvironment { variant: None, strict: Some(strict) };
        assert_eq!(
            resolve_edit_mode(None, &settings, env, Some(&model), true).unwrap(),
            pair(EditMode::Replace, EditMode::Replace),
            "{strict}"
        );
    }
    let env = EditModeEnvironment { variant: Some("hashline"), strict: None };
    // Target cfgEditMode env provenance returns before adaptive conversion.
    assert_eq!(
        resolve_edit_mode(None, &settings, env, Some(&model), true).unwrap(),
        pair(EditMode::Hashline, EditMode::Hashline)
    );
    settings.model_variants = vec![("deepseek".into(), json!("hashline"))];
    assert_eq!(
        resolve_edit_mode(None, &settings, empty, Some(&model), true).unwrap(),
        pair(EditMode::Hashline, EditMode::Hashline)
    );
    let env = EditModeEnvironment { variant: Some("patch"), strict: None };
    // b91 EditTool resolves live modelVariants before environment for both consumers.
    assert_eq!(
        resolve_edit_mode(None, &settings, env, Some(&model), true).unwrap(),
        pair(EditMode::Hashline, EditMode::Hashline)
    );
    assert_eq!(
        resolve_edit_mode(None, &settings, env, Some(&model), false).unwrap(),
        pair(EditMode::Hashline, EditMode::Hashline)
    );
    settings.model_variants.clear();
    let automatic = EditModeEnvironment { variant: Some("auto"), strict: None };
    assert_eq!(
        resolve_edit_mode(None, &settings, automatic, Some(&model), true).unwrap(),
        pair(EditMode::Replace, EditMode::Replace)
    );
}

#[test]
fn exact_modes_ordered_variants_catalog_identity_and_string_setting_family() {
    let model = json!({"provider":"Custom", "id":"ignored-execution-id", "catalogModelId":"deepseek-chat"});
    let empty = EditModeEnvironment::default();
    for mode in [EditMode::Replace, EditMode::Patch, EditMode::Hashline, EditMode::ApplyPatch, EditMode::Sloppy] {
        let settings = EditModeSettings {
            mode: Some(json!("patch")),
            model_variants: vec![("DEEPSEEK".into(), json!(mode.as_str()))],
        };
        assert_eq!(resolve_edit_mode(None, &settings, empty, Some(&model), true).unwrap(), pair(mode, mode));
        let invalid_env = EditModeEnvironment { variant: Some("not-a-mode"), strict: None };
        assert_eq!(
            resolve_edit_mode(Some(mode), &settings, invalid_env, Some(&model), true).unwrap(),
            pair(mode, mode)
        );
    }
    let settings = EditModeSettings {
        mode: Some(json!("replace")),
        model_variants: vec![
            ("DeepSeek".into(), json!(false)),
            ("deepseek".into(), json!("HASHLINE")),
            ("ignored-execution-id".into(), json!("sloppy")),
            ("CUSTOM/DEEPSEEK".into(), json!("apply_patch")),
            ("chat".into(), json!("patch")),
        ],
    };
    assert_eq!(
        resolve_edit_mode(None, &settings, empty, Some(&model), true).unwrap(),
        pair(EditMode::ApplyPatch, EditMode::ApplyPatch)
    );
    let empty_pattern = EditModeSettings { mode: None, model_variants: vec![(String::new(), json!("sloppy"))] };
    assert_eq!(
        resolve_edit_mode(None, &empty_pattern, empty, Some(&model), true).unwrap(),
        pair(EditMode::Sloppy, EditMode::Sloppy)
    );
    for raw in [json!("patch"), json!(["patch"])] {
        let settings = EditModeSettings { mode: Some(raw), model_variants: vec![] };
        assert_eq!(
            resolve_edit_mode(None, &settings, empty, Some(&model), true).unwrap(),
            pair(EditMode::Patch, EditMode::Patch)
        );
    }
    let malformed = json!({"provider":"custom", "id":"wire-model", "catalogModelId":false});
    assert!(
        resolve_edit_mode(None, &EditModeSettings::default(), empty, Some(&malformed), true)
            .unwrap_err()
            .to_string()
            .contains("catalog model ID")
    );
}

#[test]
fn invalid_captured_environment_is_not_a_native_selection_fallback() {
    let model = json!({"provider":"custom", "id":"deepseek-chat"});
    let settings = EditModeSettings { mode: None, model_variants: vec![("deepseek".into(), json!("patch"))] };
    for variant in ["Hashline", "unknown", "\" hashline \"", "\"hashline", "\"unknown\""] {
        let env = EditModeEnvironment { variant: Some(variant), strict: None };
        // Old constructor invalid-env error is superseded by b91 registry
        // env parsing: unknown enum is unset, so the model variant still wins.
        assert_eq!(resolve_edit_mode(None,&settings,env,Some(&model),true).unwrap(),pair(EditMode::Patch,EditMode::Patch));
        assert_eq!(resolve_edit_mode(None,&EditModeSettings::default(),env,Some(&model),true).unwrap(),
            pair(EditMode::Replace,EditMode::Replace),"invalid env is unset, so configured/default hashline still adapts");
        assert_eq!(
            resolve_edit_mode(None, &settings, env, Some(&model), false).unwrap(),
            pair(EditMode::Patch, EditMode::Patch)
        );
    }
    for variant in [" hashline ", "\t\"hashline\"\n", "\u{feff}hashline\u{a0}", "\"hash\\u006cine\""] {
        let env = EditModeEnvironment { variant: Some(variant), strict: None };
        assert_eq!(resolve_edit_mode(None,&EditModeSettings::default(),env,Some(&model),true).unwrap(),
            pair(EditMode::Hashline,EditMode::Hashline),"valid registry env must bypass adaptive default resolution");
        assert_eq!(resolve_edit_mode(None,&settings,env,Some(&model),true).unwrap(),
            pair(EditMode::Patch,EditMode::Patch),"matching model variant retains precedence over env");
    }
}

// Fixed edit-mode.test.ts nine original rows, including all three DeepSeek IDs.
#[test]
fn fixed_omp_nine_edit_modes_and_constructor_settings_family() {
    let empty = EditModeEnvironment::default();
    let settings = EditModeSettings { mode: Some(json!("hashline")), model_variants: vec![] };
    for (provider, id, expected) in [
        ("openrouter", "moonshotai/Kimi-K2-Instruct", EditMode::Replace),
        ("xiaomi", "MiMo-V2.5-Pro", EditMode::Replace),
        ("tensormesh", "deepseek-ai/DeepSeek-V4-Flash", EditMode::Replace),
        ("deepseek", "deepseek-chat", EditMode::Replace),
        ("deepseek", "deepseek-reasoner", EditMode::Replace),
        ("kilo", "stepfun/step-3.7-flash:free", EditMode::Replace),
        ("moonshot", "moonshot-v1-128k", EditMode::Hashline),
    ] {
        let model = json!({"provider":provider, "id":id});
        assert_eq!(
            resolve_edit_mode(None, &settings, empty, Some(&model), true).unwrap(),
            pair(expected, expected),
            "{provider}/{id}"
        );
    }
    for (selector,expected) in [
        ("kimi-code/k3",EditMode::Replace),("kimi-coding/K3-256K",EditMode::Replace),
        ("custom/k30",EditMode::Hashline),("custom/k3-custom",EditMode::Hashline),
        ("kimi-code/k3-256k-custom",EditMode::Hashline),("openai/gpt-5.5",EditMode::Hashline),
        ("openai-codex/gpt-5.3-codex-spark",EditMode::Replace),("openai-codex/gpt-5.3-codex",EditMode::Hashline),
        ("openrouter/minimax/minimax-m1",EditMode::Replace),("minimax/MiniMax-M2.5",EditMode::Replace),("minimax/MiniMax-M3",EditMode::Replace),
        ("zai/glm-5.3-flash",EditMode::Replace),("zai/glm-5.3",EditMode::Hashline),("zai/glm-4.7-flash",EditMode::Hashline),
    ] {
        let (provider,id)=selector.split_once('/').unwrap();let model=json!({"provider":provider,"id":id});
        assert_eq!(resolve_edit_mode(None,&settings,empty,Some(&model),true).unwrap(),pair(expected,expected),"target {selector}");
    }
    // Both new fixed-taxonomy pattern-only overrides must retain their
    // identities, even though Qwen and an unknown model both use hashline.
    // An outcome-only mode assertion would miss a discarded override.
    let taxonomy: serde_json::Value = serde_json::from_str(include_str!("../data/omp-edit-taxonomy.json")).unwrap();
    for (id, class, revision) in [
        ("prismml/Bonsai-27B-GGUF-Q4", "qwen", Some("3.6.0")),
        ("prismml/Ternary-Bonsai-2-27B-Q8", "qwen", Some("3.8.0")),
        ("prismml/Bonsai-27", "unknown", None),
        ("prismml/Bonsai-2-26B", "unknown", None),
    ] {
        let identity = ara_cli::catalog_rules::classify_over_taxonomy(
            &taxonomy, "", id, ara_cli::catalog_rules::ClassifyOptions { observed_at_ms: None, lenient: true },
        ).unwrap();
        assert_eq!(identity["class"], class, "{id}");
        assert_eq!(identity.get("revision").and_then(serde_json::Value::as_str), revision, "{id}");
        let model = json!({"provider":"prismml", "id":id});
        assert_eq!(resolve_edit_mode(None,&settings,empty,Some(&model),true).unwrap(),
            pair(EditMode::Hashline,EditMode::Hashline),"pattern {id}");
    }
    // Preserve exact/provider precedence, specificity, lenient ties and expiry
    // in this existing coarse bank; no extra libtest process/compile target.
    let mut ranked = taxonomy.clone();
    ranked["classes"][0]["overrides"] = json!([
        {"id":"pattern-broad","glob":"*alias*","class":"qwen","revision":"3.6.0"},
        {"id":"pattern-narrow","glob":"*alias-special*","class":"qwen","revision":"3.8.0"},
        {"id":"pattern-scoped","provider":"p","glob":"*alias*","class":"deepseek"},
        {"id":"exact-agnostic","model":"alias-exact","class":"mimo"},
        {"id":"exact-scoped","provider":"p","model":"alias-exact","class":"minimax"},
        {"id":"pattern-expired","glob":"*alias-expired*","class":"kimi","expiresAtMs":10}
    ]);
    for (provider, id, observed, class, revision) in [
        ("p", "alias-exact", None, "minimax", None),
        ("other", "alias-exact", None, "mimo", None),
        ("p", "alias-special", None, "deepseek", None),
        ("other", "alias-special", None, "qwen", Some("3.8.0")),
        ("other", "alias-expired", Some(10.0), "qwen", Some("3.6.0")),
        ("other", "alias-expired", None, "kimi", None),
    ] {
        let identity = ara_cli::catalog_rules::classify_over_taxonomy(&ranked,provider,id,
            ara_cli::catalog_rules::ClassifyOptions { observed_at_ms: observed, lenient: true }).unwrap();
        assert_eq!(identity["class"], class, "{provider}/{id}");
        assert_eq!(identity.get("revision").and_then(serde_json::Value::as_str), revision, "{provider}/{id}");
    }
    ranked["classes"][0]["overrides"].as_array_mut().unwrap().push(
        json!({"id":"pattern-tied","glob":"*alias-special*","class":"glm"}));
    let lenient = ara_cli::catalog_rules::classify_over_taxonomy(&ranked,"other","alias-special",
        ara_cli::catalog_rules::ClassifyOptions { observed_at_ms: None, lenient: true }).unwrap();
    assert_eq!(lenient["class"], "unknown");
    assert!(ara_cli::catalog_rules::classify_over_taxonomy(&ranked,"other","alias-special",
        ara_cli::catalog_rules::ClassifyOptions { observed_at_ms: None, lenient: false }).unwrap_err().0.contains("ambiguous override"));
    let kimi = json!({"provider":"openrouter", "id":"moonshotai/Kimi-K2-Instruct"});
    let variants =
        EditModeSettings { mode: Some(json!("hashline")), model_variants: vec![("Kimi-K2".into(), json!("hashline"))] };
    assert_eq!(
        resolve_edit_mode(None, &variants, empty, Some(&kimi), true).unwrap(),
        pair(EditMode::Hashline, EditMode::Hashline)
    );
    let env = EditModeEnvironment { variant: Some("hashline"), strict: None };
    assert_eq!(
        resolve_edit_mode(None, &settings, env, Some(&kimi), true).unwrap(),
        pair(EditMode::Hashline, EditMode::Hashline)
    );
    let configured = EditModeSettings { mode: Some(json!("apply_patch")), model_variants: vec![] };
    assert_eq!(
        resolve_edit_mode(None, &configured, empty, Some(&kimi), true).unwrap(),
        pair(EditMode::ApplyPatch, EditMode::ApplyPatch)
    );
    let strict = EditModeEnvironment { variant: None, strict: Some("1") };
    assert_eq!(
        resolve_edit_mode(None, &settings, strict, Some(&kimi), true).unwrap(),
        pair(EditMode::Hashline, EditMode::Hashline)
    );

    use ara_tools::edit::EditSettings;
    use policy::{EditSettingsEnvironment, EditSettingsSource, resolve_edit_settings};
    use yaml_rust2::Yaml;
    let configured = EditSettings {
        allow_fuzzy: false,
        fuzzy_threshold: 0.8,
        enforce_seen_lines: true,
        block_auto_generated: false,
    };
    let raw_settings = EditSettingsSource {
        allow_fuzzy: Some(Yaml::Boolean(false)),
        fuzzy_threshold: Some(Yaml::Real("0.8".into())),
        enforce_seen_lines: Some(Yaml::Boolean(true)),
        block_auto_generated: Some(Yaml::Boolean(false)),
    };
    assert_eq!(resolve_edit_settings(&raw_settings, EditSettingsEnvironment::default()).unwrap(), configured);
    for (fuzzy, expected) in [("true", true), ("1", true), ("false", false), ("0", false), ("auto", false)] {
        let resolved =
            resolve_edit_settings(&raw_settings, EditSettingsEnvironment { fuzzy: Some(fuzzy), fuzzy_threshold: None })
                .unwrap();
        assert_eq!(resolved.allow_fuzzy, expected);
        assert_eq!(resolved.fuzzy_threshold, 0.8);
        assert!(resolved.enforce_seen_lines);
        assert!(!resolved.block_auto_generated);
    }
    for (raw, expected) in
        [("0", 0.0), ("1", 1.0), (" .75tail", 0.75), ("8e-1suffix", 0.8), ("0x10", 0.0), ("auto", 0.8)]
    {
        let resolved =
            resolve_edit_settings(&raw_settings, EditSettingsEnvironment { fuzzy: None, fuzzy_threshold: Some(raw) })
                .unwrap();
        assert_eq!(resolved.fuzzy_threshold, expected, "{raw}");
    }
    for raw in ["", "TRUE", " false ", "unknown"] {
        let resolved=resolve_edit_settings(&raw_settings,EditSettingsEnvironment {fuzzy:Some(raw),fuzzy_threshold:None}).unwrap();
        assert!(!resolved.allow_fuzzy,"b91 parseEditFuzzyEnv({raw:?}) defers to the configured false");
        assert_eq!(resolved.fuzzy_threshold,0.8);
    }
    for raw in ["", "NaN", "Infinity", "-0.1", "1.1", "unknown"] {
        let resolved=resolve_edit_settings(&raw_settings,EditSettingsEnvironment {fuzzy:None,fuzzy_threshold:Some(raw)}).unwrap();
        assert_eq!(resolved.fuzzy_threshold,0.8,"b91 invalid threshold env {raw:?} is unset");
    }
    assert!(EditSettings::default().enforce_seen_lines,"b91 cfgEditEnforceSeenLines defaults true");
    let null_fuzzy = EditSettingsSource { allow_fuzzy: Some(Yaml::Null), ..Default::default() };
    let env_true = EditSettingsEnvironment { fuzzy: Some("true"), fuzzy_threshold: None };
    assert!(resolve_edit_settings(&null_fuzzy, env_true).unwrap().allow_fuzzy);
    assert!(
        resolve_edit_settings(&null_fuzzy, EditSettingsEnvironment::default())
            .unwrap_err()
            .to_string()
            .contains("edit.fuzzyMatch must be a boolean")
    );
    let bad_threshold = EditSettingsSource { fuzzy_threshold: Some(Yaml::String("bad".into())), ..Default::default() };
    let env_threshold = EditSettingsEnvironment { fuzzy: None, fuzzy_threshold: Some("0.9") };
    assert_eq!(resolve_edit_settings(&bad_threshold, env_threshold).unwrap().fuzzy_threshold, 0.9);
    assert!(
        resolve_edit_settings(&bad_threshold, EditSettingsEnvironment::default())
            .unwrap_err()
            .to_string()
            .contains("edit.fuzzyThreshold must be a number")
    );
    let before = ara_tools::ToolContext::new(".").with_edit_settings(configured);
    let retained = before.clone();
    let after =
        before.with_edit_settings(EditSettings::default()).with_edit_modes(EditMode::Patch, EditMode::Replace, true);
    assert_eq!(retained.edit_settings(), configured);
    assert_eq!(after.edit_settings(), EditSettings::default());
}
