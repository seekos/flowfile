use anyhow::{Context as _, Result};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::Duration,
};
use zeroize::Zeroize as _;

const KEYCHAIN_SERVICE: &str = "cc.bso.flowfile.smb";
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const KEYCHAIN_OPERATION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SmbCredential {
    version: u8,
    pub username: String,
    pub password: String,
}

static SESSION_CREDENTIALS: OnceLock<Mutex<HashMap<String, Option<SmbCredential>>>> =
    OnceLock::new();

impl Drop for SmbCredential {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}

#[derive(Serialize)]
struct SmbCredentialRef<'a> {
    version: u8,
    username: &'a str,
    password: &'a str,
}

pub(crate) fn load(server: &str) -> Result<Option<SmbCredential>> {
    let account = keychain_account(server);
    let session_cache = session_credentials()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(credential) = session_cache.get(&account) {
        return Ok(credential.clone());
    }
    drop(session_cache);

    let server = server.to_string();
    let cached_account = account.clone();
    run_keychain_operation(
        "读取 macOS 钥匙串超时",
        KEYCHAIN_OPERATION_TIMEOUT,
        move || {
            let credential = load_from_keychain(&server);
            if let Ok(credential) = &credential {
                // This closure can finish after the foreground timeout while
                // the user is responding to a macOS authorization dialog.
                // Preserve that result so the next directory click does not
                // start another Keychain request.
                session_credentials()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .entry(cached_account)
                    .or_insert_with(|| credential.clone());
            }
            credential
        },
    )
}

fn load_from_keychain(server: &str) -> Result<Option<SmbCredential>> {
    let account = keychain_account(server);
    let mut payload = match get_generic_password(KEYCHAIN_SERVICE, &account) {
        Ok(payload) => payload,
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(None),
        Err(error) => {
            return Err(anyhow::anyhow!(error)).context("无法从 macOS 钥匙串读取 SMB 凭证");
        }
    };
    let credential = decode(&payload);
    payload.zeroize();
    match credential {
        Ok(credential) if credential.version == 1 => Ok(Some(credential)),
        Ok(_) => {
            delete_from_keychain(server)?;
            Ok(None)
        }
        Err(_) => {
            delete_from_keychain(server)?;
            Ok(None)
        }
    }
}

pub(crate) fn save(server: &str, username: &str, password: &str) -> Result<()> {
    session_credentials()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            keychain_account(server),
            Some(SmbCredential {
                version: 1,
                username: username.to_string(),
                password: password.to_string(),
            }),
        );

    let server = server.to_string();
    let username = username.to_string();
    let mut password = password.to_string();
    run_keychain_operation(
        "写入 macOS 钥匙串超时",
        KEYCHAIN_OPERATION_TIMEOUT,
        move || {
            let result = save_to_keychain(&server, &username, &password);
            password.zeroize();
            result
        },
    )
}

fn save_to_keychain(server: &str, username: &str, password: &str) -> Result<()> {
    let mut payload = serde_json::to_vec(&SmbCredentialRef {
        version: 1,
        username,
        password,
    })
    .context("无法编码 SMB 凭证")?;
    let result = set_generic_password(KEYCHAIN_SERVICE, &keychain_account(server), &payload)
        .map_err(anyhow::Error::new)
        .context("无法将 SMB 凭证保存到 macOS 钥匙串");
    payload.zeroize();
    result
}

pub(crate) fn delete(server: &str) -> Result<()> {
    session_credentials()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(keychain_account(server), None);

    let server = server.to_string();
    run_keychain_operation(
        "删除 macOS 钥匙串凭证超时",
        KEYCHAIN_OPERATION_TIMEOUT,
        move || delete_from_keychain(&server),
    )
}

fn delete_from_keychain(server: &str) -> Result<()> {
    match delete_generic_password(KEYCHAIN_SERVICE, &keychain_account(server)) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(error) => Err(anyhow::anyhow!(error)).context("无法删除失效的 SMB 钥匙串凭证"),
    }
}

fn run_keychain_operation<T: Send + 'static>(
    timeout_message: &'static str,
    timeout: Duration,
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("flowfile-keychain".to_string())
        .spawn(move || {
            let _ = sender.send(operation());
        })
        .context("无法启动 macOS 钥匙串任务")?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => anyhow::bail!(timeout_message),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            anyhow::bail!("macOS 钥匙串任务意外终止")
        }
    }
}

fn decode(payload: &[u8]) -> Result<SmbCredential> {
    serde_json::from_slice(payload).context("无法解码 SMB 凭证")
}

fn keychain_account(server: &str) -> String {
    server.trim_end_matches('.').to_ascii_lowercase()
}

fn session_credentials() -> &'static Mutex<HashMap<String, Option<SmbCredential>>> {
    SESSION_CREDENTIALS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::{
        SmbCredentialRef, decode, delete, keychain_account, load, run_keychain_operation, save,
    };
    use std::{thread, time::Duration};

    #[test]
    fn credential_payload_round_trips_non_ascii_values() {
        let payload = serde_json::to_vec(&SmbCredentialRef {
            version: 1,
            username: "办公室\\张三",
            password: "密码 with spaces",
        })
        .unwrap();
        let credential = decode(&payload).unwrap();

        assert_eq!(credential.version, 1);
        assert_eq!(credential.username, "办公室\\张三");
        assert_eq!(credential.password, "密码 with spaces");
    }

    #[test]
    fn keychain_accounts_normalize_server_case_and_trailing_dot() {
        assert_eq!(keychain_account("NAS.Local."), "nas.local");
    }

    #[test]
    fn keychain_operations_have_a_deadline() {
        let error =
            run_keychain_operation("测试钥匙串超时", Duration::from_millis(20), || {
                thread::sleep(Duration::from_millis(100));
                Ok(())
            })
            .expect_err("operation should time out");

        assert_eq!(error.to_string(), "测试钥匙串超时");
    }

    #[test]
    #[ignore = "writes and removes an isolated credential in the macOS login keychain"]
    fn live_keychain_round_trip() {
        let server = format!("flowfile-keychain-test-{}.invalid", std::process::id());
        delete(&server).unwrap();
        save(&server, "测试用户", "temporary-secret").unwrap();
        let credential = load(&server).unwrap().expect("stored credential");
        assert_eq!(credential.username, "测试用户");
        assert_eq!(credential.password, "temporary-secret");
        drop(credential);
        delete(&server).unwrap();
        assert!(load(&server).unwrap().is_none());
    }
}
