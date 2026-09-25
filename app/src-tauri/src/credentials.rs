#[derive(Clone, Copy)]
pub enum CredentialKind {
    Soniox,
    DeepSeek,
    Doubao,
    Bailian,
    ElevenLabs,
}

impl CredentialKind {
    fn account(self) -> &'static str {
        match self {
            Self::Soniox => "soniox-api-key",
            Self::DeepSeek => "deepseek-api-key",
            Self::Doubao => "doubao-speech-api-key",
            Self::Bailian => "bailian-api-key",
            Self::ElevenLabs => "elevenlabs-api-key",
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
const SERVICE: &str = "com.lectureedit.desktop";

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn entry_for(service: &str, account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(service, account).map_err(|_| "无法访问系统安全凭据".to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn entry(kind: CredentialKind) -> Result<keyring::Entry, String> {
    entry_for(SERVICE, kind.account())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn load(kind: CredentialKind) -> Result<Option<String>, String> {
    match entry(kind)?.get_password() {
        Ok(secret) if secret.is_empty() => Ok(None),
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("无法读取系统安全凭据".into()),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn save(kind: CredentialKind, secret: Option<&str>) -> Result<(), String> {
    let entry = entry(kind)?;
    match secret {
        Some(secret) => entry
            .set_password(secret)
            .map_err(|_| "无法保存到系统安全凭据".to_string()),
        None => match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("无法从系统安全凭据中删除密钥".into()),
        },
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn load(_kind: CredentialKind) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn save(_kind: CredentialKind, _secret: Option<&str>) -> Result<(), String> {
    Err("当前系统暂不支持安全保存 API Key".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_accounts_are_stable_and_service_specific() {
        assert_eq!(CredentialKind::Soniox.account(), "soniox-api-key");
        assert_eq!(CredentialKind::DeepSeek.account(), "deepseek-api-key");
        let accounts = [
            CredentialKind::Soniox,
            CredentialKind::DeepSeek,
            CredentialKind::Doubao,
            CredentialKind::Bailian,
            CredentialKind::ElevenLabs,
        ]
        .map(CredentialKind::account);
        let unique: std::collections::HashSet<_> = accounts.iter().collect();
        assert_eq!(unique.len(), accounts.len());
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    #[ignore = "uses the native operating-system credential store"]
    fn native_credential_store_round_trip() {
        let service = format!("com.lectureedit.desktop.test.{}", uuid::Uuid::new_v4());
        let entry = entry_for(&service, "round-trip").unwrap();
        entry.set_password("temporary-secret").unwrap();
        assert_eq!(entry.get_password().unwrap(), "temporary-secret");
        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
    }
}
