//! Property 47: configuration expansion does not leak values (42.2, 42.8).
use std::collections::BTreeMap;

use ara_agent_lite_mcp::{
    config::{expand_server, expand_str},
    ConfigError, McpConfig, McpOptions, McpSession, ServerConfig, ServerStatus,
};
use proptest::prelude::*;
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Reference of Claude Code `expandEnvVarsInString`, written against the
/// regex `\$\{([^}]+)\}` and JavaScript `split(':-', 2)`.
fn reference(template: &str, env: &BTreeMap<String, String>) -> (String, Vec<String>) {
    let chars: Vec<char> = template.chars().collect();
    let (mut out, mut missing, mut i) = (String::new(), Vec::<String>::new(), 0);
    while i < chars.len() {
        if chars[i] == '$' && chars.get(i + 1) == Some(&'{') {
            if let Some(len) = chars[i + 2..].iter().position(|&c| c == '}').filter(|&len| len > 0) {
                let content: String = chars[i + 2..i + 2 + len].iter().collect();
                let parts: Vec<&str> = content.split(":-").take(2).collect();
                match (env.get(parts[0]), parts.get(1)) {
                    (Some(value), _) => out.push_str(value),
                    (None, Some(default)) => out.push_str(default),
                    (None, None) => {
                        if !missing.contains(&parts[0].to_string()) {
                            missing.push(parts[0].to_string());
                        }
                        out.push_str(&format!("${{{content}}}"));
                    }
                }
                i += len + 3;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    (out, missing)
}

fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z /:.=-]{0,6}",
        Just("${".to_string()),
        Just("}".to_string()),
        Just(":-".to_string()),
        "\\$\\{(VAR_A|VAR_B|UNSET_C|)\\}",
        "\\$\\{(VAR_A|UNSET_C|UNSET_D):-[a-z:-]{0,6}\\}",
    ]
}

fn template() -> impl Strategy<Value = String> {
    proptest::collection::vec(piece(), 0..8).prop_map(|pieces| pieces.concat())
}

/// Secret-looking values from an alphabet no template or name uses.
fn secret() -> impl Strategy<Value = String> {
    "[QWXZ]{10,16}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn expansion_matches_reference(t in template(), a in secret(), b in prop_oneof![Just(String::new()), secret()]) {
        let env = BTreeMap::from([("VAR_A".to_string(), a), ("VAR_B".to_string(), b)]);
        let lookup = |name: &str| env.get(name).cloned();
        let (mut missing, mut used) = (Vec::new(), Vec::new());
        let expanded = expand_str(&t, &lookup, &mut missing, &mut used);
        let (expected, expected_missing) = reference(&t, &env);
        prop_assert_eq!(expanded, expected);
        prop_assert_eq!(missing, expected_missing);
    }

    #[test]
    fn errors_name_only_servers_and_variables(
        t in template(), a in secret(), header in secret(), server in "[a-z]{1,8}",
    ) {
        let env = BTreeMap::from([("VAR_A".to_string(), a.clone()), ("VAR_B".to_string(), "QQQQQQQQQQQQ".to_string())]);
        let lookup = |name: &str| env.get(name).cloned();
        let config = ServerConfig::Http {
            url: format!("http://127.0.0.1:1/{t}"),
            headers: BTreeMap::from([("Authorization".to_string(), format!("Bearer {header} {t}"))]),
        };
        // url and header carry the same template after fixed `$`-free text.
        let expected_missing = reference(&t, &env).1;
        match expand_server(&server, &config, &lookup) {
            Ok(ServerConfig::Http { url, headers }) => {
                prop_assert!(expected_missing.is_empty());
                prop_assert_eq!(url, format!("http://127.0.0.1:1/{}", reference(&t, &env).0));
                prop_assert!(headers["Authorization"].contains(&header));
            }
            Ok(_) => prop_assert!(false),
            Err(error) => {
                let text = error.to_string();
                prop_assert_eq!(error.clone(), ConfigError::MissingVariable { server: server.clone(), variables: expected_missing });
                prop_assert!(!text.contains(&a) && !text.contains(&header) && !text.contains("QQQQ"), "{}", text);
            }
        }
        let debug = format!("{:?}", config);
        prop_assert!(!debug.contains(&header));
    }
}

#[test]
fn unsupported_transports_and_fields_are_explicit() {
    let parse = |server: serde_json::Value| McpConfig::from_value(&json!({"mcpServers": {"s": server}}));
    for transport in ["sse", "ws", "sse-ide", "ws-ide", "sdk", "claudeai-proxy"] {
        let error = parse(json!({"type": transport, "url": "https://x"})).unwrap_err();
        assert_eq!(error, ConfigError::UnsupportedTransport { server: "s".into(), transport: transport.into() });
        assert_eq!(error.code(), "unsupported_transport");
    }
    for field in ["oauth", "headersHelper"] {
        let error = parse(json!({"type":"http","url":"https://x", field: {"clientId": "TOPSECRETVALUE"}})).unwrap_err();
        assert_eq!(error, ConfigError::UnsupportedField { server: "s".into(), field: field.into() });
        assert!(!error.to_string().contains("TOPSECRETVALUE"));
    }
    let error = parse(json!({"command":"x","cwd":"/tmp"})).unwrap_err();
    assert_eq!(error.code(), "unsupported_field");
    assert!(McpConfig::from_value(&json!({"mcpServers": {}, "other": 1})).is_err());
    let error = parse(json!({"command": 3})).unwrap_err();
    assert_eq!(error.code(), "invalid_config");
    let parsed = McpConfig::from_json_str(
        r#"{"mcpServers":{"local":{"command":"python","args":["s.py"],"env":{"K":"${K:-v}"}},"remote":{"type":"http","url":"https://h/mcp","headers":{"X-Key":"${KEY}"}}}}"#,
    )
    .unwrap();
    assert!(matches!(&parsed.mcp_servers["local"], ServerConfig::Stdio { command, .. } if command == "python"));
    assert!(
        matches!(&parsed.mcp_servers["remote"], ServerConfig::Http { headers, .. } if headers["X-Key"] == "${KEY}")
    );
    assert!(McpConfig::from_json_str("{not json SECRET").unwrap_err().to_string().find("SECRET").is_none());
}

#[tokio::test]
async fn invalid_http_values_never_reach_status() {
    let secret = "QWXZQWXZQWXZ-url-secret";
    let config = McpConfig {
        mcp_servers: BTreeMap::from([
            ("badurl".to_string(), ServerConfig::Http { url: format!("::{secret}"), headers: BTreeMap::new() }),
            (
                "badheader".to_string(),
                ServerConfig::Http {
                    url: "http://127.0.0.1:9/mcp".into(),
                    headers: BTreeMap::from([("X-Key".to_string(), format!("{secret}\n"))]),
                },
            ),
        ]),
    };
    let session = McpSession::connect(config, McpOptions::default(), CancellationToken::new()).await.unwrap();
    for status in session.status().values() {
        assert!(matches!(status, ServerStatus::Failed { reason } if !reason.contains(secret)), "{status:?}");
    }
}
