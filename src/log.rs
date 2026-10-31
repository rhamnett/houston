//! A small log file at ~/Library/Logs/houston.log. When the app is started
//! from Finder or a login item its stderr goes nowhere, so stderr is pointed
//! at the file. Run from a terminal, it stays on the terminal.

use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

/// Keep at most this much before starting the file again.
const MAX_BYTES: u64 = 1_000_000;

fn path() -> Option<PathBuf> {
    // A sandboxed instance logs into its own data folder.
    if let Some(dir) = std::env::var_os("HOUSTON_DATA_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("houston.log"));
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs/houston.log"))
}

pub fn init() {
    if std::io::stderr().is_terminal() {
        return;
    }
    let Some(path) = path() else { return };
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES);
    let file = OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(&path);
    if let Ok(file) = file {
        // SAFETY: duplicating a valid descriptor we own onto stderr.
        unsafe {
            libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
        }
        line(&format!("started {}", env!("CARGO_PKG_VERSION")));
    }
}

/// One timestamped line on stderr (the log file when not on a terminal).
pub fn line(msg: &str) {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let _ = writeln!(std::io::stderr(), "{now} {msg}");
}
