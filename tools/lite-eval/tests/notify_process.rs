//! Real PowerShell process with a substitute script: the argument array
//! reaches the script unchanged (Chinese, quotes, backticks, newlines), the
//! receipt is parsed, and a pre-network refusal is classified as not sent.
//! The real webhook script is never called here.
#![cfg(windows)]

use std::{fs, path::Path};

use lite_eval::{
    common::Credentials,
    notify::{Notice, Notifier, ProcessRunner, Severity, Status, SENDER, STARTED_AT},
};

const ECHO: &str = r#"[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Title,
    [Parameter(Mandatory = $true)][string]$Message,
    [string]$Progress,
    [string]$OverallProgress,
    [DateTimeOffset]$StartedAt = [DateTimeOffset]::MinValue,
    [ValidateSet('Info', 'Milestone', 'Decision', 'Blocker')][string]$Severity = 'Info',
    [string]$Sender
)
$ErrorActionPreference = 'Stop'
if ($Title -eq 'refuse') { throw 'Notification exceeds the local 16 KiB limit; send a concise summary.' }
$seen = [ordered]@{ title = $Title; message = $Message; progress = $Progress; overall = $OverallProgress;
    started = $StartedAt.ToString('o'); severity = $Severity; sender = $Sender }
[IO.File]::WriteAllText($env:LITE_EVAL_ECHO_OUT, ($seen | ConvertTo-Json -Compress), (New-Object Text.UTF8Encoding $false))
@{ timestamp = 'now'; title = $Title; delivered = $true; errcode = '0' } | ConvertTo-Json -Compress
"#;

fn notice(title: &str) -> Notice {
    Notice {
        severity: Severity::Milestone,
        title: title.to_owned(),
        message: "- **证据**：`gates/a \"b\".json`\n- 未覆盖：无 😀 \\ end".to_owned(),
        progress: "进度台账 5/5".to_owned(),
        overall_progress: "verified 检查点 0/36".to_owned(),
    }
}

#[test]
fn arguments_survive_a_real_powershell_process() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("echo.ps1");
    // PowerShell 5.1 reads a BOM-less script as ANSI; the BOM keeps it UTF-8.
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(ECHO.as_bytes());
    fs::write(&script, bytes).unwrap();
    let out = dir.path().join("seen.json");
    std::env::set_var("LITE_EVAL_ECHO_OUT", &out);
    let notifier =
        Notifier::new(dir.path().join("notifications.jsonl"), script.clone(), ProcessRunner, Credentials::default());

    let sent = notice("M-X-NOTIFY 钉钉进度通知：完成");
    let record = notifier.send(&sent, "digest-1", Some(4)).unwrap().expect("first notice is sent");
    assert_eq!(record.status, Status::Delivered, "{record:?}");
    let seen: serde_json::Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(seen["title"], sent.title);
    assert_eq!(seen["message"], sent.message);
    assert_eq!(seen["progress"], sent.progress);
    assert_eq!(seen["overall"], sent.overall_progress);
    assert_eq!(seen["severity"], "Milestone");
    assert_eq!(seen["sender"], SENDER);
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(seen["started"].as_str().unwrap()).unwrap(),
        chrono::DateTime::parse_from_rfc3339(STARTED_AT).unwrap()
    );

    assert!(notifier.send(&sent, "digest-1", Some(4)).unwrap().is_none(), "same digest is not resent");
    let refused = notifier.send(&notice("refuse"), "digest-2", Some(5)).unwrap().unwrap();
    assert_eq!((refused.status, refused.reason_category.as_deref()), (Status::NotSent, Some("too_large")));
    assert!(notifier.send(&notice("refuse"), "digest-2", Some(5)).unwrap().is_none(), "failed content is not resent");
    let log = fs::read_to_string(dir.path().join("notifications.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 2);
    assert!(!log.contains("access_token="));
    assert!(Path::new(&out).exists());
}
