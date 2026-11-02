//! Secrets live in the login Keychain (service `local.houston`), never in
//! config.json. The config structs still carry them in memory, so sources
//! don't need to know; [`crate::config::Config::save`] blanks them before
//! writing, and Preferences only ever learns whether each one is set.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

pub const SERVICE: &str = "local.houston";

/// `HOUSTON_KEYCHAIN_SERVICE` swaps the service, so a sandboxed instance (the
/// end-to-end tests) never touches the real secrets.
fn service() -> String {
    std::env::var("HOUSTON_KEYCHAIN_SERVICE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| SERVICE.to_string())
}
/// errSecItemNotFound.
const NOT_FOUND: i32 = -25300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secret {
    /// Bearer token for the local HTTP API.
    ApiToken,
    LinearApiKey,
    /// Reads releases of the private repository, for the update check.
    GithubToken,
}

impl Secret {
    pub const ALL: [Secret; 3] = [Secret::ApiToken, Secret::LinearApiKey, Secret::GithubToken];

    /// The Keychain account name, e.g. for
    /// `security find-generic-password -s local.houston -a api-token -w`.
    pub fn account(self) -> &'static str {
        match self {
            Secret::ApiToken => "api-token",
            Secret::LinearApiKey => "linear-api-key",
            Secret::GithubToken => "github-token",
        }
    }
}

pub fn get(secret: Secret) -> Option<String> {
    get_raw(secret.account())
}

/// Store `value`, or remove the item when it is empty.
pub fn set(secret: Secret, value: &str) -> Result<(), String> {
    set_raw(secret.account(), value)
}

/// The Keychain account holding the secret half of an IAM access key.
pub fn aws_secret_account(access_key_id: &str) -> String {
    format!("aws-secret:{access_key_id}")
}

/// `AKIA…` (long-lived) or `ASIA…` (temporary), 16 to 128 capitals and digits.
pub fn is_access_key_id(id: &str) -> bool {
    (16..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

pub fn get_aws_secret(access_key_id: &str) -> Option<String> {
    get_raw(&aws_secret_account(access_key_id))
}

pub fn set_aws_secret(access_key_id: &str, value: &str) -> Result<(), String> {
    if !is_access_key_id(access_key_id) {
        return Err(format!("{access_key_id} is not an access key id"));
    }
    set_raw(&aws_secret_account(access_key_id), value)
}

/// `HOUSTON_INSECURE_SECRETS_FILE`: keep secrets in this JSON file (mode
/// 600) instead of the Keychain. Only for CI runners, whose Keychain can't be
/// used unattended; never for real use.
fn insecure_file() -> Option<std::path::PathBuf> {
    std::env::var_os("HOUSTON_INSECURE_SECRETS_FILE")
        .filter(|p| !p.is_empty())
        .map(Into::into)
}

type FileStore = std::collections::BTreeMap<String, String>;

fn read_file(path: &std::path::Path) -> FileStore {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_file(path: &std::path::Path, store: &FileStore) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let json = serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// What each Keychain account held when first read, or was last set to.
/// CloudWatch wants its secret key on every poll, and each read of an item
/// can ask for the login password (a rebuilt binary isn't yet trusted), so
/// the Keychain is asked once per launch. A refused read is remembered too:
/// asking again every minute would only repeat the prompt. Other failures
/// (a locked Keychain after sleep) are not: the next read tries again.
static CACHE: std::sync::Mutex<std::collections::BTreeMap<String, Option<String>>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

fn get_raw(account: &str) -> Option<String> {
    if let Some(path) = insecure_file() {
        return read_file(&path).remove(account).filter(|s| !s.is_empty());
    }
    // Held across the read, so two sources wanting one secret get one prompt.
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(value) = cache.get(account) {
        return value.clone();
    }
    let (value, settled) = read_keychain(account);
    if settled {
        cache.insert(account.to_string(), value.clone());
    }
    value
}

/// errSecUserCanceled and errSecAuthFailed: the prompt was dismissed or denied.
const REFUSED: [i32; 2] = [-128, -25293];

/// The value, and whether that answer is final (found, absent or refused).
fn read_keychain(account: &str) -> (Option<String>, bool) {
    match get_generic_password(&service(), account) {
        Ok(bytes) => (
            String::from_utf8(bytes).ok().filter(|s| !s.is_empty()),
            true,
        ),
        Err(e) if e.code() == NOT_FOUND => (None, true),
        Err(e) => {
            crate::log::line(&format!("keychain read {account}: {e}"));
            (None, REFUSED.contains(&e.code()))
        }
    }
}

fn set_raw(account: &str, value: &str) -> Result<(), String> {
    if let Some(path) = insecure_file() {
        let mut store = read_file(&path);
        if value.is_empty() {
            store.remove(account);
        } else {
            store.insert(account.to_string(), value.to_string());
        }
        return write_file(&path, &store);
    }
    let result = if value.is_empty() {
        match delete_generic_password(&service(), account) {
            Err(e) if e.code() == NOT_FOUND => Ok(()),
            other => other,
        }
    } else {
        set_generic_password(&service(), account, value.as_bytes())
    };
    result.map_err(|e| format!("keychain {account}: {e}"))?;
    let stored = Some(value.to_string()).filter(|v| !v.is_empty());
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(account.to_string(), stored);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_key_ids() {
        assert!(is_access_key_id("AKIAIOSFODNN7EXAMPLE"));
        assert!(!is_access_key_id("akiaiosfodnn7example"));
        assert!(!is_access_key_id("AKIA-SHORT"));
        assert_eq!(aws_secret_account("AKIAX"), "aws-secret:AKIAX");
    }
}
