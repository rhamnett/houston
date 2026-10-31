//! Houston's version comes from git tags, not Cargo.toml: releases are cut by
//! CI from Conventional Commits (scripts/conventional.sh), and a bot can't
//! commit a version bump to the protected main branch.
//!
//! - `HOUSTON_VERSION` if set (the release job sets it from the new tag)
//! - else `git describe`: `0.2.0` on a tag, `0.2.0+3.gabc1234` three commits
//!   past it (semver build metadata, so the update check still sees 0.2.0)
//! - else `0.0.0-unversioned` (a source archive has no git history), with a
//!   build warning; the update check then knows not to offer itself

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

fn describe() -> Option<String> {
    // Releases only: a pre-release tag (v1.0.0-rc.1) is never "the last release".
    let text = git(&[
        "describe",
        "--tags",
        "--long",
        "--match",
        "v[0-9]*",
        "--exclude",
        "*-*",
    ])?;
    // v0.2.0-3-gabc1234: the tag may contain hyphens, so split from the right.
    let mut parts = text.rsplitn(3, '-');
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
    // A new commit or tag changes the answer. Ask git where these live: in a
    // linked worktree `.git` is a file, not the directory.
    let mut watch = vec!["HEAD".to_string(), "refs/tags".into(), "packed-refs".into()];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(branch);
    }
    for name in &watch {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && std::path::Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let version = std::env::var("HOUSTON_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty())
        .or_else(describe)
        .unwrap_or_else(|| {
            println!(
                "cargo:warning=no release tag or HOUSTON_VERSION: building as 0.0.0-unversioned"
            );
            "0.0.0-unversioned".to_string()
        });
    println!("cargo:rustc-env=HOUSTON_VERSION={version}");
}
