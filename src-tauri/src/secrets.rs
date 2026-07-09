//! API Key 密钥存储抽象（技术文档 5.3）。
//!
//! Key 明文只进系统钥匙串；为便于测试（headless CI 无 keychain），抽象为 trait，
//! 生产用 [`KeyringStore`]，测试用内存实现。Key 本体永不进 DB / 日志。

// MemoryStore 供测试与后续命令测试使用；先落地。
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::AppResult;

const SERVICE: &str = "com.gendesk.app";

pub trait SecretStore: Send + Sync {
    fn set(&self, account: &str, secret: &str) -> AppResult<()>;
    fn get(&self, account: &str) -> AppResult<Option<String>>;
    fn delete(&self, account: &str) -> AppResult<()>;
}

/// 生产实现：系统钥匙串（macOS Keychain / Windows 凭据管理器）。
pub struct KeyringStore;

impl SecretStore for KeyringStore {
    fn set(&self, account: &str, secret: &str) -> AppResult<()> {
        keyring::Entry::new(SERVICE, account)?.set_password(secret)?;
        Ok(())
    }

    fn get(&self, account: &str) -> AppResult<Option<String>> {
        match keyring::Entry::new(SERVICE, account)?.get_password() {
            Ok(pw) => Ok(Some(pw)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn delete(&self, account: &str) -> AppResult<()> {
        match keyring::Entry::new(SERVICE, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// 测试 / 无钥匙串环境的内存实现。
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn set(&self, account: &str, secret: &str) -> AppResult<()> {
        if let Ok(mut m) = self.map.lock() {
            m.insert(account.to_string(), secret.to_string());
        }
        Ok(())
    }

    fn get(&self, account: &str) -> AppResult<Option<String>> {
        Ok(self.map.lock().ok().and_then(|m| m.get(account).cloned()))
    }

    fn delete(&self, account: &str) -> AppResult<()> {
        if let Ok(mut m) = self.map.lock() {
            m.remove(account);
        }
        Ok(())
    }
}

/// 计算脱敏后缀：`****后4位`（不足 4 位时全遮）。
pub fn mask(secret: &str) -> String {
    let n = secret.chars().count();
    if n <= 4 {
        "****".to_string()
    } else {
        let last4: String = secret.chars().skip(n - 4).collect();
        format!("****{last4}")
    }
}
