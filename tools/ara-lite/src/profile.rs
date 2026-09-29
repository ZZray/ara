//! Profile and join files: where the service is and which credential to use
//! (Go: `readConnection`, `savePrivate`, `configDir` in `main.go`).

use crate::err;
use crate::error::{Error, Result};
use crate::types::Connection;
use crate::util::random_hex;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

/// Default data directory root: `<user config dir>/ara-lite-rs`, or `.ara-lite-rs` when unknown.
///
/// It is deliberately not the Go tool's `<user config dir>/ara-lite`: both keep a `state.db`
/// there, and opening the live Go database would write to it.
pub fn config_dir() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    };
    data_dir_in(base)
}

fn data_dir_in(base: Option<PathBuf>) -> PathBuf {
    match base {
        Some(dir) => dir.join("ara-lite-rs"),
        None => PathBuf::from(".ara-lite-rs"),
    }
}

/// Create a directory tree readable only by the current user where the platform allows it.
pub fn create_private_dir(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|e| err!("create {}: {e}", path.display()))
}

fn private_file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Write `value` as indented JSON via a private temp file. `exclusive` publishes with a
/// hard link so an existing file is never replaced; otherwise the file is replaced.
pub fn save_private(path: &Path, value: &Connection, exclusive: bool) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    create_private_dir(&parent)?;
    let tmp = parent.join(format!(".ara-lite-{}.tmp", random_hex(8)?));
    let written = (|| -> std::io::Result<()> {
        let mut file = private_file_options().open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    let published = if exclusive { fs::hard_link(&tmp, path) } else { fs::rename(&tmp, path) };
    // After a rename the temp name is gone; after a link it must be removed.
    let _ = fs::remove_file(&tmp);
    published.map_err(Error::from)
}

/// Parse `http://<loopback-ip>[:port]` with no userinfo, path, query or fragment.
pub fn loopback_addr(url: &str) -> Result<SocketAddr> {
    let bad = || Error::new("profile must point to a loopback HTTP service");
    let rest = url.strip_prefix("http://").ok_or_else(bad)?;
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', '\\']) {
        return Err(bad());
    }
    let (host, port) = if let Some(bracketed) = rest.strip_prefix('[') {
        let (host, tail) = bracketed.split_once(']').ok_or_else(bad)?;
        let port = match tail {
            "" => 80,
            t => t.strip_prefix(':').ok_or_else(bad)?.parse::<u16>().map_err(|_| bad())?,
        };
        (host, port)
    } else {
        match rest.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| bad())?),
            None => (rest, 80),
        }
    };
    let ip: IpAddr = host.parse().map_err(|_| bad())?;
    if !ip.to_canonical().is_loopback() {
        return Err(bad());
    }
    Ok(SocketAddr::new(ip, port))
}

/// Read and validate a profile or join file (Go: `readConnection`).
pub fn read_connection(path: &Path) -> Result<Connection> {
    let bytes = fs::read(path).map_err(|e| err!("open {}: {e}", path.display()))?;
    let connection: Connection = serde_json::from_slice(&bytes)?;
    loopback_addr(&connection.url)?;
    if connection.token.is_empty() {
        return Err(Error::new("profile has no credential; join again"));
    }
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_urls_are_validated() {
        assert!(loopback_addr("http://127.0.0.1:7342").is_ok());
        assert!(loopback_addr("http://[::1]:7342").is_ok());
        assert!(loopback_addr("http://127.0.0.1").is_ok());
        for bad in [
            "https://127.0.0.1:1",
            "http://localhost:1",
            "http://10.0.0.1:1",
            "http://user@127.0.0.1:1",
            "http://127.0.0.1:1/",
            "http://127.0.0.1:1/api",
            "http://127.0.0.1:1?x=1",
            "http://127.0.0.1:1#f",
            "http://127.0.0.1:notaport",
            "",
        ] {
            assert!(loopback_addr(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn default_data_dir_is_not_the_go_tools_directory() {
        // Go `configDir` (main.go): `<user config dir>/ara-lite`, or `.ara-lite` when unknown.
        let base = PathBuf::from("config");
        assert_eq!(data_dir_in(Some(base.clone())), base.join("ara-lite-rs"));
        assert_ne!(data_dir_in(Some(base.clone())), base.join("ara-lite"));
        assert_eq!(data_dir_in(None), PathBuf::from(".ara-lite-rs"));
        assert_ne!(data_dir_in(None), PathBuf::from(".ara-lite"));
        let live = config_dir();
        let name = live.file_name().map(|n| n.to_string_lossy().into_owned());
        assert!(matches!(name.as_deref(), Some("ara-lite-rs" | ".ara-lite-rs")), "{}", live.display());
    }

    #[test]
    fn save_private_refuses_to_replace_when_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let value = Connection {
            url: "http://127.0.0.1:1".into(),
            token: "t".into(),
            id: String::new(),
            role: "worker".into(),
        };
        save_private(&path, &value, true).unwrap();
        assert!(save_private(&path, &value, true).is_err());
        save_private(&path, &value, false).unwrap();
        let back = read_connection(&path).unwrap();
        assert_eq!(back, value);
        // No temp files are left behind.
        let leftovers: Vec<_> =
            fs::read_dir(dir.path()).unwrap().flatten().filter(|e| e.file_name() != "p.json").collect();
        assert!(leftovers.is_empty());
    }
}
