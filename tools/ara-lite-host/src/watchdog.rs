//! Opt-in supervision of this process's own Worker child only.
//!
//! A stale but live Worker is reported and remains owned until it exits or the
//! operator requests a stop. It is never killed merely for a missing heartbeat.
//! A clean exit is completion; failed exits may restart within a fixed budget.
//! Task reconciliation, Session fences and tool replay are outside this module.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::File;

const STOP: &str = "intentional-stop";
const HEALTH: &str = "health.json";
const STATUS: &str = "watchdog-status.json";
static WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const REPLACE_ATTEMPTS: usize = 8;
const REPLACE_RETRY_MS: u64 = 5;

#[derive(Clone, Debug)]
pub struct Config {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub state_dir: PathBuf,
    pub poll_ms: u64,
    pub stale_ms: u64,
    pub max_restarts: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub starts: usize,
    pub restarts: usize,
    /// `intentional_stop`, `child_exited`, `restart_limit`, or
    /// `stalled_child_exited`. No returned Report leaves an owned child running.
    pub why: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Health {
    pid: u32,
    at_unix_ms: u64,
    phase: String,
}

#[derive(Serialize)]
struct Status<'a> {
    state: &'a str,
    pid: Option<u32>,
    starts: usize,
    restarts: usize,
    at_unix_ms: u64,
    detail: &'a str,
}

fn now_ms() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis().try_into()?)
}

/// Windows can transiently deny replacement while another handle does not
/// share delete access (observed WinError 5 in the native heartbeat fixture).
/// Retry only that boundary, for at most 35 ms. Never delete the old target or
/// turn an unpublished heartbeat into a new liveness observation.
fn replace_with_retry(mut replace: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    for attempt in 0..REPLACE_ATTEMPTS {
        match replace() {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    && attempt + 1 < REPLACE_ATTEMPTS =>
            {
                thread::sleep(Duration::from_millis(REPLACE_RETRY_MS));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the final replacement attempt always returns")
}

/// Both the data and its replacement are committed before success. A unique
/// temporary file prevents concurrent heartbeat/status writers sharing a name.
fn save_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("state file needs a parent directory")?;
    fs::create_dir_all(parent)?;
    let sequence = WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".watchdog-{}-{sequence}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        drop(file);
        replace_with_retry(|| fs::rename(&temporary, path)).context("publish watchdog state by atomic replacement")?;
        // Opening directories is unsupported through std::fs::File on Windows.
        // The file was synced before the atomic replacement on both platforms.
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Publish Worker liveness. A phase such as waiting_human is ordinary health,
/// not an error or a reason to restart an in-flight Agent.
pub fn heartbeat(state_dir: &Path, phase: &str) -> Result<()> {
    ensure!(!phase.trim().is_empty() && phase.len() <= 128, "invalid heartbeat phase");
    save_json(&state_dir.join(HEALTH), &Health { pid: std::process::id(), at_unix_ms: now_ms()?, phase: phase.into() })
}

/// An operator stop survives supervisor restarts. It stops only the child owned
/// by that supervisor; it never resets a Task or removes a reconciliation fence.
pub fn request_stop(state_dir: &Path) -> Result<()> {
    save_json(&state_dir.join(STOP), &serde_json::json!({"at_unix_ms":now_ms()?}))
}

/// Explicit operator start only. `supervise` never clears an intentional stop.
pub fn clear_stop(state_dir: &Path) -> Result<()> {
    match fs::remove_file(state_dir.join(STOP)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn stopped(state_dir: &Path) -> Result<bool> {
    match fs::metadata(state_dir.join(STOP)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Cleanup on an error must not abandon a live owned child. Normal stale
/// handling keeps this guard alive and waits, rather than triggering cleanup.
struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn status(config: &Config, report: &Report, state: &str, pid: Option<u32>, detail: &str) -> Result<()> {
    save_json(
        &config.state_dir.join(STATUS),
        &Status { state, pid, starts: report.starts, restarts: report.restarts, at_unix_ms: now_ms()?, detail },
    )
}

/// Blocking, opt-in native supervision. Only a nonzero/signaled child exit may
/// restart. A live stale child latches `stalled`: automatic recovery stays off
/// even if its heartbeat returns; the operator can stop it or let it finish.
pub fn supervise(config: &Config) -> Result<Report> {
    ensure!(config.program.is_absolute() && config.program.is_file(), "absolute existing Worker program required");
    ensure!(config.state_dir.is_absolute(), "absolute watchdog state directory required");
    ensure!((10..=60_000).contains(&config.poll_ms), "poll_ms must be 10..60000");
    ensure!(
        config.stale_ms > config.poll_ms && config.stale_ms <= 86_400_000,
        "stale_ms must exceed poll_ms and be at most one day"
    );
    ensure!(config.max_restarts <= 32, "max_restarts must be at most 32");
    ensure!(
        config.args.len() <= 128 && config.args.iter().map(String::len).sum::<usize>() <= 64 * 1024,
        "Worker arguments exceed limits"
    );
    fs::create_dir_all(&config.state_dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(config.state_dir.join("watchdog.lock"))?;
    lock.try_lock_exclusive().context("another watchdog owns this state directory")?;
    let mut report = Report { starts: 0, restarts: 0, why: String::new() };
    loop {
        if stopped(&config.state_dir)? {
            report.why = "intentional_stop".into();
            status(config, &report, "stopped", None, "Operator stop prevents launch and recovery")?;
            return Ok(report);
        }
        // stdio remains inherited so the reference Host's operator JSONL input
        // and output remain usable. No machine service or detached monitor.
        let mut child = OwnedChild(
            Command::new(&config.program)
                .args(&config.args)
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .context("launch Worker")?,
        );
        report.starts += 1;
        report.restarts = report.starts - 1;
        let pid = child.0.id();
        let mut last_heartbeat = Instant::now(); // bounded startup grace
        let mut stalled = false;
        let mut healthy_reported = false;
        let mut health_detail = String::from("Waiting for the owned Worker's first heartbeat");
        status(config, &report, "starting", Some(pid), &health_detail)?;
        loop {
            if stopped(&config.state_dir)? {
                // This is an explicit operator stop, not a liveness inference.
                if child.0.try_wait()?.is_none() {
                    child.0.kill().context("stop owned Worker")?;
                }
                child.0.wait().context("join stopped Worker")?;
                report.why = "intentional_stop".into();
                status(
                    config,
                    &report,
                    "stopped",
                    None,
                    "Owned Worker joined; Task effects still require normal reconciliation",
                )?;
                return Ok(report);
            }
            if let Some(exit) = child.0.try_wait().context("observe owned Worker exit")? {
                if stalled {
                    report.why = "stalled_child_exited".into();
                    status(config, &report, "stalled", None, &health_detail)?;
                    return Ok(report);
                }
                if exit.success() {
                    report.why = "child_exited".into();
                    status(config, &report, "exited", None, "Worker exited successfully; no restart")?;
                    return Ok(report);
                }
                if report.restarts == config.max_restarts {
                    report.why = "restart_limit".into();
                    status(config, &report, "failed", None, "Failed Worker exit exhausted the restart budget")?;
                    return Ok(report);
                }
                break;
            }
            if !stalled {
                let health = (|| -> Result<Health> {
                    let mut bytes = Vec::new();
                    let mut options = OpenOptions::new();
                    options.read(true);
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::OpenOptionsExt;
                        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE.
                        // A heartbeat publisher must be able to replace the file
                        // while this bounded read owns its original handle.
                        options.share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004);
                    }
                    let mut file = options.open(config.state_dir.join(HEALTH))?.take(4097);
                    file.read_to_end(&mut bytes)?;
                    drop(file); // No read handle survives parsing or polling.
                    ensure!(bytes.len() <= 4096, "heartbeat exceeds 4096 bytes");
                    Ok(serde_json::from_slice::<Health>(&bytes)?)
                })();
                match health {
                    Ok(health) if health.pid == pid && !health.phase.trim().is_empty() && health.phase.len() <= 128 => {
                        let now = now_ms()?;
                        if health.at_unix_ms <= now {
                            let age = Duration::from_millis(now - health.at_unix_ms);
                            if age <= Duration::from_millis(config.stale_ms) {
                                last_heartbeat = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
                                health_detail = format!("Worker phase: {}", health.phase);
                                if !healthy_reported {
                                    status(config, &report, "healthy", Some(pid), &health_detail)?;
                                    healthy_reported = true;
                                }
                            } else {
                                health_detail = "Owned Worker heartbeat is stale".into();
                            }
                        } else {
                            health_detail = "Owned Worker heartbeat timestamp is in the future".into();
                        }
                    }
                    Ok(_) => {
                        health_detail = "Heartbeat does not identify the owned Worker or has an invalid phase".into()
                    }
                    Err(_) => health_detail = "Owned Worker heartbeat is missing or unreadable".into(),
                }
                if last_heartbeat.elapsed() >= Duration::from_millis(config.stale_ms) {
                    stalled = true;
                    status(config, &report, "stalled", Some(pid), &health_detail)?;
                }
            }
            thread::sleep(Duration::from_millis(config.poll_ms));
        }
        // The original child has exited and been reaped before this delay. Check
        // the persistent stop on every slice and again immediately before launch.
        drop(child);
        let delay = Duration::from_millis(config.poll_ms.saturating_mul(1 << report.restarts.min(6)).min(1000));
        let until = Instant::now() + delay;
        status(config, &report, "backoff", None, "Failed Worker exited; bounded restart pending")?;
        while Instant::now() < until {
            if stopped(&config.state_dir)? {
                report.why = "intentional_stop".into();
                status(config, &report, "stopped", None, "Operator stop prevents restart")?;
                return Ok(report);
            }
            thread::sleep(Duration::from_millis(config.poll_ms).min(until.saturating_duration_since(Instant::now())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::mpsc;

    fn python() -> PathBuf {
        let output = Command::new("python")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .expect("native watchdog fixtures require the installed Python");
        assert!(output.status.success());
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    }

    fn config(state: &Path, script: &str) -> Config {
        Config {
            program: python(),
            args: vec!["-c".into(), script.into(), state.to_string_lossy().into_owned()],
            state_dir: state.into(),
            poll_ms: 10,
            stale_ms: 1000,
            max_restarts: 2,
        }
    }

    const WRITER: &str = "import os,json,time,pathlib,sys\np=pathlib.Path(sys.argv[1])\ndef beat(phase,pid=None):\n t=p/'child-health.tmp'\n with t.open('w') as output:\n  json.dump({'pid':os.getpid() if pid is None else pid,'at_unix_ms':int(time.time()*1000),'phase':phase},output)\n  output.flush()\n  os.fsync(output.fileno())\n for attempt in range(8):\n  try:\n   os.replace(t,p/'health.json')\n   return\n  except OSError as error:\n   if os.name!='nt' or getattr(error,'winerror',None) not in (5,32,33) or attempt==7:\n    raise RuntimeError('heartbeat was not published; last successful timestamp remains unchanged') from error\n   time.sleep(.005)\n";

    #[cfg(windows)]
    #[test]
    fn atomic_replace_recovers_after_a_real_windows_delete_sharing_denial() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("health.json");
        let temporary = directory.path().join("replacement.tmp");
        fs::write(&target, b"old heartbeat").unwrap();
        fs::write(&temporary, b"new heartbeat").unwrap();
        let mut blocker =
            Some(OpenOptions::new().read(true).share_mode(0x0000_0001 | 0x0000_0002).open(&target).unwrap());
        let mut attempts = 0;
        replace_with_retry(|| {
            attempts += 1;
            let result = fs::rename(&temporary, &target);
            if attempts == 1 {
                let error = result.as_ref().expect_err("the owned deny-delete handle must prevent replacement");
                assert!(matches!(error.raw_os_error(), Some(5 | 32 | 33)), "{error}");
                // Release only after the actual OS denial: this test does not
                // depend on a timing race or relax the healthy-child contract.
                drop(blocker.take());
            }
            result
        })
        .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(fs::read(target).unwrap(), b"new heartbeat");
        assert!(!temporary.exists());
    }

    #[cfg(windows)]
    #[test]
    fn replacement_retry_is_bounded_and_other_errors_are_not_retried() {
        let mut attempts = 0;
        let error = replace_with_retry(|| {
            attempts += 1;
            Err(std::io::Error::from_raw_os_error(5))
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(5));
        assert_eq!(attempts, REPLACE_ATTEMPTS);
        attempts = 0;
        let error = replace_with_retry(|| {
            attempts += 1;
            Err(std::io::Error::from_raw_os_error(2))
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(2));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn failed_native_child_has_a_finite_restart_budget() {
        let directory = tempfile::tempdir().unwrap();
        let result = supervise(&config(directory.path(), "import sys; sys.exit(7)")).unwrap();
        assert_eq!(result.starts, 3);
        assert_eq!(result.restarts, 2);
        assert_eq!(result.why, "restart_limit");
    }

    #[test]
    fn intentional_stop_prevents_launch_and_child_exit_cannot_revive_it() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(
            directory.path(),
            "import pathlib,sys; pathlib.Path(sys.argv[1],'intentional-stop').write_text('stop'); sys.exit(7)",
        );
        request_stop(directory.path()).unwrap();
        let stopped = supervise(&config).unwrap();
        assert_eq!(stopped.starts, 0);
        assert_eq!(stopped.why, "intentional_stop");
        clear_stop(directory.path()).unwrap();
        let stopped = supervise(&config).unwrap();
        assert_eq!(stopped.starts, 1);
        assert_eq!(stopped.restarts, 0);
        assert_eq!(stopped.why, "intentional_stop");
    }

    #[test]
    fn fresh_waiting_human_heartbeats_are_not_failures() {
        let directory = tempfile::tempdir().unwrap();
        let script = format!("{WRITER}\nfor _ in range(60):\n beat('waiting_human')\n time.sleep(.02)\n");
        let mut config = config(directory.path(), &script);
        config.stale_ms = 500;
        let result = supervise(&config).unwrap();
        assert_eq!(result.starts, 1);
        assert_eq!(result.restarts, 0);
        assert_eq!(result.why, "child_exited");
    }

    #[test]
    fn stale_live_child_is_reported_without_kill_or_restart() {
        let directory = tempfile::tempdir().unwrap();
        let script =
            format!("{WRITER}\nbeat('running')\ntime.sleep(.35)\n(p/'finished-without-kill').write_text('settled')\n");
        let mut config = config(directory.path(), &script);
        config.stale_ms = 100;
        let result = supervise(&config).unwrap();
        assert_eq!(result.starts, 1);
        assert_eq!(result.restarts, 0);
        assert_eq!(result.why, "stalled_child_exited");
        assert_eq!(fs::read(directory.path().join("finished-without-kill")).unwrap(), b"settled");
    }

    #[test]
    fn another_pids_heartbeat_does_not_keep_owned_child_healthy() {
        let directory = tempfile::tempdir().unwrap();
        let script = format!("{WRITER}\nfor _ in range(15):\n beat('ready',1)\n time.sleep(.02)\n");
        let mut config = config(directory.path(), &script);
        config.stale_ms = 100;
        let result = supervise(&config).unwrap();
        assert_eq!(result.starts, 1);
        assert_eq!(result.restarts, 0);
        assert_eq!(result.why, "stalled_child_exited");
    }

    #[test]
    fn explicit_stop_joins_only_the_supervisors_owned_child() {
        let directory = tempfile::tempdir().unwrap();
        let script = format!("{WRITER}\nfor _ in range(100):\n beat('waiting_human')\n time.sleep(.02)\n");
        let config = config(directory.path(), &script);
        let (send, receive) = mpsc::channel();
        let worker = thread::spawn(move || send.send(supervise(&config)).unwrap());
        let until = Instant::now() + Duration::from_secs(3);
        while !directory.path().join(HEALTH).is_file() && Instant::now() < until {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(directory.path().join(HEALTH).is_file());
        request_stop(directory.path()).unwrap();
        let result = receive.recv_timeout(Duration::from_secs(4)).unwrap().unwrap();
        worker.join().unwrap();
        assert_eq!(result.starts, 1);
        assert_eq!(result.restarts, 0);
        assert_eq!(result.why, "intentional_stop");
    }

    #[test]
    fn heartbeat_replacement_and_invalid_config_are_explicit() {
        let directory = tempfile::tempdir().unwrap();
        heartbeat(directory.path(), "running").unwrap();
        heartbeat(directory.path(), "ready").unwrap();
        let health: Value = serde_json::from_slice(&fs::read(directory.path().join(HEALTH)).unwrap()).unwrap();
        assert_eq!(health["pid"], std::process::id());
        assert_eq!(health["phase"], "ready");
        assert!(heartbeat(directory.path(), " ").is_err());
        let mut config = config(directory.path(), "import sys; sys.exit(0)");
        config.poll_ms = 0;
        assert!(supervise(&config).is_err());
    }
}
