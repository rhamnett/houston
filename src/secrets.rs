//! Secrets live in the login Keychain (service `local.houston`), never in
//! config.json. The config structs still carry them in memory, so sources
//! don't need to know; [`crate::config::Config::save`] blanks them before
//! writing, and Preferences only ever learns whether each one is set.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

pub const SERVICE: &str = "local.houston";
/// errSecItemNotFound.
const NOT_FOUND: i32 = -25300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secret {
    /// Bearer token for the local HTTP API.
    ApiToken,
    LinearApiKey,
}

impl Secret {
    pub const ALL: [Secret; 2] = [Secret::ApiToken, Secret::LinearApiKey];

    /// The Keychain account name, e.g. for
    /// `security find-generic-password -s local.houston -a api-token -w`.
    pub fn account(self) -> &'static str {
        match self {
            Secret::ApiToken => "api-token",
            Secret::LinearApiKey => "linear-api-key",
        }
    }
}

pub fn get(secret: Secret) -> Option<String> {
    match get_generic_password(SERVICE, secret.account()) {
        Ok(bytes) => String::from_utf8(bytes).ok().filter(|s| !s.is_empty()),
        Err(e) => {
            if e.code() != NOT_FOUND {
                crate::log::line(&format!("keychain read {}: {e}", secret.account()));
            }
            None
        }
    }
}

/// Store `value`, or remove the item when it is empty.
pub fn set(secret: Secret, value: &str) -> Result<(), String> {
    let result = if value.is_empty() {
        match delete_generic_password(SERVICE, secret.account()) {
            Err(e) if e.code() == NOT_FOUND => Ok(()),
            other => other,
        }
    } else {
        set_generic_password(SERVICE, secret.account(), value.as_bytes())
    };
    result.map_err(|e| format!("keychain {}: {e}", secret.account()))
}
