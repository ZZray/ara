//! `ara-lite serve` (Go: `serve`, `joinCommands` in `main.go`).
//!
//! The Go service can show a terminal console; this port prints a plain status block
//! (or one JSON readiness line) instead. The TUI is deferred, see `docs/simplification-proposals.md`.

use crate::err;
use crate::error::{Error, Result};
use crate::profile::{config_dir, save_private};
use crate::server::Server;
use crate::store::Store;
use crate::types::{Connection, State};
use crate::util::has_nonprintable;
use clap::Parser;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const DEFAULT_ADDR: &str = "127.0.0.1:7342";
/// How often leases and expired messages are persisted, independent of HTTP traffic.
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Parser, Debug)]
#[command(no_binary_name = true, disable_help_flag = true, disable_version_flag = true)]
struct ServeArgs {
    /// Loopback listen address.
    #[arg(long, default_value = DEFAULT_ADDR, allow_hyphen_values = true)]
    addr: String,
    /// Persistent data directory.
    #[arg(long = "data-dir", allow_hyphen_values = true)]
    data_dir: Option<PathBuf>,
    /// Plain text status (the default; kept for command line compatibility).
    #[arg(long)]
    plain: bool,
    /// Print one JSON readiness event instead of the status text.
    #[arg(long)]
    json: bool,
}

/// Quote for a POSIX shell (Go: `shellQuote`).
fn quote_posix(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Quote for PowerShell.
fn quote_powershell(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The copy-paste join commands shown at startup (Go: `joinCommands`). `windows`
/// selects PowerShell quoting and the `& ` call prefix.
pub fn join_commands(exe: &Path, dir: &Path, state: &State, windows: bool) -> String {
    let quote = |s: &str| if windows { quote_powershell(s) } else { quote_posix(s) };
    let prefix = if windows { "& " } else { "" };
    let master_name = state.clients.values().find(|c| c.role == "master").map(|c| c.name.clone()).unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    for role in ["master", "worker"] {
        // First profile name not yet present in the data directory.
        let mut n = 1;
        let profile_name = loop {
            let candidate = format!("{role}-{n}");
            if std::fs::symlink_metadata(dir.join(format!("{candidate}.json"))).is_err() {
                break candidate;
            }
            n += 1;
        };
        let name = if role == "master" && !master_name.is_empty() { master_name.clone() } else { profile_name.clone() };
        if has_nonprintable(&name) {
            lines.push(
                "The master already joined and its name has non-printable characters, so a recovery command \
                 cannot be shown safely. Reuse the saved master profile."
                    .to_string(),
            );
            continue;
        }
        lines.push(format!(
            "{role} join:\n{prefix}{} join --role {role} --name {} --join-file {} --profile {}",
            quote(&exe.to_string_lossy()),
            quote(&name),
            quote(&dir.join(format!("{role}-join.json")).to_string_lossy()),
            quote(&dir.join(format!("{profile_name}.json")).to_string_lossy()),
        ));
    }
    format!("{}\n\nMore workers: use another name and profile, reusing worker-join.json.", lines.join("\n\n"))
}

fn loopback_listen_addr(text: &str) -> Result<SocketAddr> {
    let bad = || Error::new("--addr must use a loopback IP, e.g. 127.0.0.1:7342");
    let addr: SocketAddr = text.parse().map_err(|_| bad())?;
    if !addr.ip().to_canonical().is_loopback() {
        return Err(bad());
    }
    Ok(addr)
}

/// Run the service until it fails; there is no signal handler, Ctrl+C ends the process.
/// The database uses rollback-journal mode with full sync, so an abrupt stop is safe.
pub fn serve(args: &[String]) -> Result<()> {
    let a = ServeArgs::try_parse_from(args).map_err(|e| {
        let rendered = e.render().to_string();
        Error::new(rendered.lines().next().unwrap_or_default().trim_start_matches("error: ").to_string())
    })?;
    if a.plain && a.json {
        return Err(Error::new("choose --plain or --json"));
    }
    let addr = loopback_listen_addr(&a.addr)?;
    let store = Arc::new(Store::open(&a.data_dir.unwrap_or_else(config_dir))?);
    let listener = TcpListener::bind(addr).map_err(|e| err!("listen: {e}; choose another --addr port"))?;
    let address = format!("http://{}", listener.local_addr()?);
    let dir = store.dir().to_path_buf();
    let (master_key, worker_key) = store.join_keys();
    let master_join = dir.join("master-join.json");
    let worker_join = dir.join("worker-join.json");
    for (path, role, key) in [(&master_join, "master", master_key), (&worker_join, "worker", worker_key)] {
        save_private(
            path,
            &Connection { url: address.clone(), token: key, id: String::new(), role: role.to_string() },
            false,
        )?;
    }
    let exe = std::env::current_exe()?;
    let join_help = join_commands(&exe, &dir, &store.snapshot()?, cfg!(windows));
    let server = Server::start(listener, store.clone())?;
    let (failed_tx, failed_rx) = mpsc::channel::<String>();
    {
        let store = store.clone();
        thread::Builder::new().name("ara-lite-maintenance".into()).spawn(move || {
            loop {
                thread::sleep(MAINTENANCE_INTERVAL);
                if let Err(e) = store.tick() {
                    let _ = failed_tx.send(e.to_string());
                    return;
                }
            }
        })?;
    }
    if a.json {
        let ready = serde_json::json!({
            "status": "ready",
            "url": address,
            "data_dir": dir,
            "master_join_file": master_join,
            "worker_join_file": worker_join,
        });
        println!("{ready}");
    } else {
        println!(
            "ara-lite READY\n{address}\nData: {}\n\n{join_help}\nCtrl+C stops the service. Help: ara-lite --help",
            dir.display()
        );
    }
    // Blocks until the maintenance thread reports a persistence failure.
    let reason = failed_rx.recv().unwrap_or_else(|_| "maintenance thread ended".to_string());
    server.shutdown();
    store.close();
    Err(err!("cannot persist lease state: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Client;

    fn state_with_master(name: &str) -> State {
        let mut state = State::default();
        let client = Client { id: "m1".into(), role: "master".into(), name: name.into(), ..Client::default() };
        state.clients.insert(client.id.clone(), client);
        state
    }

    #[test]
    fn join_commands_pick_free_profile_names_and_quote() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("worker-1.json"), "{}").unwrap();
        let exe = Path::new("/opt/ara lite/ara-lite");
        let text = join_commands(exe, dir.path(), &State::default(), false);
        assert!(text.contains("master join:\n'/opt/ara lite/ara-lite' join --role master --name 'master-1'"), "{text}");
        assert!(text.contains("--role worker --name 'worker-2'"), "{text}");
        assert!(text.contains("worker-2.json"), "{text}");
        assert!(text.ends_with("reusing worker-join.json."), "{text}");
        let windows = join_commands(exe, dir.path(), &State::default(), true);
        assert!(windows.contains("master join:\n& '/opt/ara lite/ara-lite' join"), "{windows}");
    }

    #[test]
    fn join_commands_reuse_the_existing_master_name_and_escape_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let text = join_commands(Path::new("x"), dir.path(), &state_with_master("it's me"), false);
        assert!(text.contains(r#"--name 'it'"'"'s me'"#), "{text}");
        let text = join_commands(Path::new("x"), dir.path(), &state_with_master("it's me"), true);
        assert!(text.contains("--name 'it''s me'"), "{text}");
    }

    #[test]
    fn unprintable_master_names_get_no_command() {
        let dir = tempfile::tempdir().unwrap();
        let text = join_commands(Path::new("x"), dir.path(), &state_with_master("bad\u{7}name"), false);
        assert!(text.contains("cannot be shown safely"), "{text}");
        assert!(!text.contains("badname"), "{text}");
        assert!(text.contains("worker join:"), "{text}");
    }

    #[test]
    fn listen_address_must_be_a_loopback_ip() {
        assert!(loopback_listen_addr("127.0.0.1:7342").is_ok());
        assert!(loopback_listen_addr("[::1]:0").is_ok());
        assert!(loopback_listen_addr("0.0.0.0:7342").is_err());
        assert!(loopback_listen_addr("localhost:7342").is_err());
        assert!(loopback_listen_addr("192.168.1.2:1").is_err());
        assert!(loopback_listen_addr("nonsense").is_err());
    }
}
