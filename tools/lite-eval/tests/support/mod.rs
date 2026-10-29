//! Helpers shared by the sealed-holdout property tests.

#![allow(dead_code)]

use std::{io, path::Path};

/// Directory link that needs no privilege: a junction on Windows (created
/// with `mklink /J`), a symbolic link elsewhere.
pub fn dir_link(link: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        // `cmd` reads a `/` inside a path argument as the start of a switch.
        let native = |path: &Path| path.to_string_lossy().replace('/', "\\");
        let output = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(native(link))
            .arg(native(target))
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "mklink /J {} {} failed: {}{}",
                link.display(),
                target.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )))
        }
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link)
    }
}

/// File symbolic link. On Windows this needs `SeCreateSymbolicLinkPrivilege`
/// or Developer Mode; the error is returned, never ignored.
pub fn file_symlink(link: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link)
    }
}
