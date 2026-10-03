//! Grouped cases ported from fixed OMP auth-broker-wire-schema-contract.test.ts.
use super::*;
use serde_json::json;

fn samples() -> Vec<(BrokerWireSchema, Value)> {
    use BrokerWireSchema as S;
    let refresher = json!({"enabled":false,"intervalMs":60000,"skewMs":300000,"nextSweepInMs":9007199254740991_u64});
    let block =
        json!({"providerKey":"anthropic:oauth","blockScope":"tier:fable","blockedUntilMs":4000,"updatedAtMs":3000});
    let remote = json!({"type":"oauth","access":"access","refresh":"__remote__","expires":5000,"tokenUrl":"https://example.test/token","clientId":"provider-client"});
    let mut real = remote.clone();
    real["refresh"] = json!("real-refresh");
    let key = json!({"type":"api_key","key":"secret","source":"login"});
    let entry = json!({"id":7,"provider":"anthropic","credential":remote,"identityKey":"account:test"});
    let mut scheduled = entry.clone();
    scheduled["rotatesInMs"] = Value::Null;
    scheduled["blocks"] = json!([block]);
    let snapshot =
        json!({"generation":2,"generatedAt":1000,"serverNowMs":2000,"refresher":refresher,"credentials":[scheduled]});
    let mut stream_snapshot = snapshot.clone();
    stream_snapshot["kind"] = json!("snapshot");
    let stream_entry =
        json!({"kind":"entry","generation":3,"serverNowMs":2500,"refresher":refresher,"entry":scheduled});
    let stream_removed = json!({"kind":"removed","generation":4,"serverNowMs":3000,"refresher":refresher,"id":7});
    let report = json!({"provider":"anthropic","fetchedAt":2000,"limits":[{"id":"rolling","label":"Rolling window","scope":{"provider":"anthropic","windowId":"rolling","providerExtension":true},"window":{"id":"rolling","label":"5 hour","durationMs":18000000},"amount":{"used":1,"limit":10,"remaining":9,"unit":"tokens","providerExtension":"kept"},"status":"ok","notes":["limit note"],"providerExtension":1}],"notes":["report note"],"metadata":{"plan":"max"},"raw":{"providerPayload":true},"providerExtension":"kept"});
    let observed = json!({"at":1000,"provider":"anthropic","model":"claude","requests":1,"inputTokens":2,"outputTokens":3,"cacheReadTokens":4,"cacheWriteTokens":5,"costUsd":0.01});
    let disabled = json!({"id":7,"provider":"anthropic","type":"oauth","email":"user@example.test","cause":"revoked","disabledAtMs":2000.5});
    vec![
        (S::OAuthCredential, real.clone()),
        (S::RemoteOAuthCredential, remote.clone()),
        (S::ApiKeyCredential, key),
        (S::WritableAuthCredential, real.clone()),
        (S::SnapshotCredential, remote),
        (S::CredentialSnapshotEntry, entry.clone()),
        (S::CredentialBlockSnapshot, block.clone()),
        (S::SnapshotEntry, scheduled),
        (S::RefresherSchedule, refresher),
        (S::SnapshotResponse, snapshot),
        (S::SnapshotStreamSnapshotEvent, stream_snapshot),
        (S::SnapshotStreamEntryEvent, stream_entry.clone()),
        (S::SnapshotStreamRemovedEvent, stream_removed),
        (S::SnapshotStreamEvent, stream_entry),
        (S::HealthzResponse, json!({"ok":true,"version":"contract"})),
        (S::UsageResponse, json!({"generatedAt":2000,"reports":[report]})),
        (
            S::UsageHistoryResponse,
            json!({"generatedAt":2000,"entries":[{"recordedAt":1000,"provider":"anthropic","accountKey":"account:test","limitId":"rolling","label":"Rolling window","usedFraction":0.1,"status":"ok"}]}),
        ),
        (
            S::ClientUsageReportRequest,
            json!({"installId":"install","hostname":"host","app":"robomp","entries":[observed]}),
        ),
        (S::ClientUsageReportResponse, json!({"ok":true})),
        (
            S::ClientUsageSummaryResponse,
            json!({"generatedAt":2000,"clients":[{"installId":"install","hostname":"host","firstSeen":1000,"lastSeen":2000,"providers":[{"app":"robomp","provider":"anthropic","requests":1,"inputTokens":2,"outputTokens":3,"cacheReadTokens":4,"cacheWriteTokens":5,"costUsd":0.01}]}]}),
        ),
        (S::CredentialRefreshResponse, json!({"entry":entry})),
        (S::CredentialDisableRequest, json!({})),
        (S::CredentialDisableResponse, json!({"ok":true})),
        (S::DisabledCredentialSummary, disabled.clone()),
        (S::DisabledCredentialsResponse, json!({"generatedAt":2000,"disabled":[disabled]})),
        (S::CredentialBlockRequest, block),
        (S::CredentialBlockResponse, json!({"ok":true})),
        (S::CredentialBlocksDeleteResponse, json!({"ok":true})),
        (S::UsageStaleResponse, json!({"ok":true})),
        (S::CredentialUploadRequest, json!({"provider":"anthropic","credential":real})),
        (S::CredentialUploadResponse, json!({"entries":[entry]})),
    ]
}

fn sample(schema: BrokerWireSchema) -> Value {
    samples().into_iter().find(|(kind, _)| *kind == schema).expect("canonical sample").1
}

#[test]
fn all_31_native_entrypoints_and_fixed_envelope_boundaries() {
    let cases = samples();
    assert_eq!(cases.len(), 31);
    for (schema, value) in cases {
        assert!(validate_wire(schema, &value).is_ok(), "{schema:?}");
        assert!(validate_wire(schema, &Value::Null).is_err(), "{schema:?}");
        if !matches!(
            schema,
            BrokerWireSchema::OAuthCredential
                | BrokerWireSchema::RemoteOAuthCredential
                | BrokerWireSchema::WritableAuthCredential
                | BrokerWireSchema::SnapshotCredential
        ) {
            let mut extra = value.clone();
            extra["extra"] = json!(true);
            assert!(validate_wire(schema, &extra).is_err(), "{schema:?}");
        }
    }
    for schema in [BrokerWireSchema::CredentialSnapshotEntry, BrokerWireSchema::SnapshotEntry] {
        let mut value = sample(schema);
        value.as_object_mut().expect("entry").remove("identityKey");
        assert!(validate_wire(schema, &value).is_err());
        value["identityKey"] = Value::Null;
        assert!(validate_wire(schema, &value).is_ok());
        value["id"] = json!(1.5);
        assert!(validate_wire(schema, &value).is_err());
    }
    let mut scheduled = sample(BrokerWireSchema::SnapshotEntry);
    scheduled.as_object_mut().expect("entry").remove("rotatesInMs");
    assert!(validate_wire(BrokerWireSchema::SnapshotEntry, &scheduled).is_err());
    let mut snapshot = sample(BrokerWireSchema::SnapshotResponse);
    snapshot["generation"] = json!(1.5);
    assert!(parse_snapshot(&snapshot).is_err());
    let mut removed = sample(BrokerWireSchema::SnapshotStreamRemovedEvent);
    removed["kind"] = json!("unknown");
    assert!(parse_stream_event(&removed).is_err());
    let mut empty_scope = sample(BrokerWireSchema::CredentialBlockRequest);
    empty_scope["blockScope"] = json!("");
    assert!(validate_wire(BrokerWireSchema::CredentialBlockSnapshot, &empty_scope).is_ok());
    assert!(validate_wire(BrokerWireSchema::CredentialBlockRequest, &empty_scope).is_ok());
}

#[test]
fn credential_extensions_sentinel_and_projection_boundaries() {
    let remote = sample(BrokerWireSchema::RemoteOAuthCredential);
    let real = sample(BrokerWireSchema::OAuthCredential);
    assert!(validate_wire(BrokerWireSchema::OAuthCredential, &remote).is_err());
    assert!(validate_wire(BrokerWireSchema::RemoteOAuthCredential, &real).is_err());
    let mut empty_refresh = real.clone();
    empty_refresh["refresh"] = json!("");
    assert!(validate_wire(BrokerWireSchema::OAuthCredential, &empty_refresh).is_ok());
    let mut empty_access = real;
    empty_access["access"] = json!("");
    assert!(validate_wire(BrokerWireSchema::OAuthCredential, &empty_access).is_err());
    let entry = parse_credential_entry(&sample(BrokerWireSchema::CredentialSnapshotEntry)).expect("entry");
    match entry.credential {
        AuthCredential::OAuth { fields } => {
            assert_eq!(fields["tokenUrl"], json!("https://example.test/token"));
            assert_eq!(fields["clientId"], json!("provider-client"));
            assert_eq!(fields["refresh"], json!(REMOTE_REFRESH_SENTINEL));
        }
        _ => panic!("expected oauth"),
    }
    let mut key = sample(BrokerWireSchema::ApiKeyCredential);
    key["source"] = json!("environment");
    assert!(validate_wire(BrokerWireSchema::ApiKeyCredential, &key).is_err());
    let tombstone = parse_disabled_summary(&sample(BrokerWireSchema::DisabledCredentialSummary)).expect("tombstone");
    assert_eq!(tombstone.disabled_at_ms, Some(2000.5));
    let empty = BrokerSnapshot::default();
    assert_eq!(empty.refresher.interval_ms, 0.0);
    assert_eq!(empty.refresher.skew_ms, 0.0);
    assert_eq!(empty.refresher.next_sweep_in_ms, 9_007_199_254_740_991.0);
    // The native integer contract sees IEEE-754 rounding, not serde's exact
    // integer token. Projection is bounded by the existing Rust i64 IDs.
    assert_eq!(integer(&json!(9_007_199_254_740_993_i64)).expect("integer"), 9_007_199_254_740_992_i64);
    assert!(integer(&json!(9_223_372_036_854_775_808_u64)).is_err());
}

#[test]
fn usage_extensions_and_native_enum_type_failures() {
    let mut response = sample(BrokerWireSchema::UsageResponse);
    assert!(validate_wire(BrokerWireSchema::UsageResponse, &response).is_ok());
    response["reports"][0]["limits"][0]["amount"]["unit"] = json!("seconds");
    assert!(validate_wire(BrokerWireSchema::UsageResponse, &response).is_err());
    response["reports"][0]["limits"][0]["amount"]["unit"] = json!("tokens");
    response["reports"][0]["limits"][0]["status"] = json!("critical");
    assert!(validate_wire(BrokerWireSchema::UsageResponse, &response).is_err());
    response["reports"][0]["limits"][0]["status"] = json!("unknown");
    response["reports"][0]["resetCredits"] =
        json!({"availableCount":2,"credits":[{"grantedAt":"today","extension":1}],"extension":true});
    assert!(validate_wire(BrokerWireSchema::UsageResponse, &response).is_ok());
    response["reports"][0]["limits"][0]["scope"]["shared"] = Value::Null;
    let error = validate_wire(BrokerWireSchema::UsageResponse, &response).expect_err("wrong type");
    assert!(!format!("{error:?} {error}").contains("today"));
    let mut report = sample(BrokerWireSchema::ClientUsageReportRequest);
    report["entries"][0]["extension"] = json!(true);
    assert!(validate_wire(BrokerWireSchema::ClientUsageReportRequest, &report).is_ok());
    report["hostname"] = Value::Null;
    assert!(validate_wire(BrokerWireSchema::ClientUsageReportRequest, &report).is_err());
}
