//! Houston's version comes from git tags, not Cargo.toml: releases are cut by
//! CI from Conventional Commits (scripts/conventional.sh), and a bot can't
//! commit a version bump to the protected main branch.
//!
//! - `HOUSTON_VERSION` if set (the release job sets it from the new tag)
//! - else `git describe`: `0.2.0` on a tag, `0.2.0+3.gabc1234` three commits
//!   past it (semver build metadata, so the update check still sees 0.2.0)
//! - else Cargo.toml's placeholder, `0.0.0`

use std::path::Path;
use std::process::Command;

fn describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--long", "--match", "v[0-9]*"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8(out.stdout).ok()?;
    // v0.2.0-3-gabc1234: the tag may contain hyphens, so split from the right.
    let mut parts = text.trim().rsplitn(3, '-');
    let (hash, ahead, tag) = (parts.next()?, parts.next()?, parts.next()?);
    let version = tag.strip_prefix('v')?;
    Some(if ahead == "0" {
        version.to_string()
    } else {
        format!("{version}+{ahead}.{hash}")
    })
}

fn main() {
    println!("cargo:rerun-if-env-changed=HOUSTON_VERSION");
    // A new commit or tag changes the answer.
    for path in [".git/HEAD", ".git/refs/tags", ".git/packed-refs"] {
        if Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Ok(head) = std::fs::read_to_string(".git/HEAD")
        && let Some(branch) = head.trim().strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=.git/{branch}");
    }
    let version = std::env::var("HOUSTON_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty())
        .or_else(describe)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=HOUSTON_VERSION={version}");
}
