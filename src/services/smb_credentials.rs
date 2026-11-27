use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use zeroize::Zeroize as _;

#[derive(Clone, Debug)]
pub(crate) struct SmbCredential {
    pub username: String,
    pub password: String,
}

static SESSION_CREDENTIALS: OnceLock<Mutex<HashMap<String, SmbCredential>>> = OnceLock::new();

impl Drop for SmbCredential {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}

pub(crate) fn load(server: &str) -> Option<SmbCredential> {
    session_credentials()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&credential_key(server))
        .cloned()
}

pub(crate) fn save(server: &str, username: &str, password: &str) {
    session_credentials()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            credential_key(server),
            SmbCredential {
                username: username.to_string(),
                password: password.to_string(),
            },
        );
}

pub(crate) fn delete(server: &str) {
    session_credentials()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&credential_key(server));
}

fn credential_key(server: &str) -> String {
    server.trim_end_matches('.').to_ascii_lowercase()
}

fn session_credentials() -> &'static Mutex<HashMap<String, SmbCredential>> {
    SESSION_CREDENTIALS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::{credential_key, delete, load, save};

    #[test]
    fn session_credentials_are_reused_and_can_be_forgotten() {
        let server = "session-credentials.test";
        save(server, "办公室\\张三", "密码 with spaces");

        let credential = load(server).expect("stored session credential");
        assert_eq!(credential.username, "办公室\\张三");
        assert_eq!(credential.password, "密码 with spaces");

        delete(server);
        assert!(load(server).is_none());
    }

    #[test]
    fn credential_keys_normalize_server_case_and_trailing_dot() {
        assert_eq!(credential_key("NAS.Local."), "nas.local");
    }
}
