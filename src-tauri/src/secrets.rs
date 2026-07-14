//! API Key 密钥存储抽象（技术文档 5.3）。
//!
//! 生产实现 [`FileStore`]：密钥落在 app data 目录下的本地加密文件
//! （`secrets.key` 主密钥 + `secrets.enc` XChaCha20-Poly1305 密文）。
//!
//! # 安全水位（如实记录）
//!
//! 本项目无 Apple 开发者证书，CI 用自签名证书，签名锚点不受信任 → macOS Keychain
//! 按应用 ACL 的「始终允许」无法跨版本存活（每次更新 / 重编译都弹授权），此为系统
//! 固有限制。故放弃系统钥匙串，改为本地加密文件。
//!
//! 无可信签名身份时，任何方案都收敛到「以当前用户身份运行的进程均可读取密钥」。
//! 本方案靠 文件权限 0600 + FileVault 全盘加密 + 文件层 XChaCha20-Poly1305：
//! **防误不防恶** —— 防的是备份 / 截图 / grep 出现明文，主密钥与密文同目录，
//! **不构成独立安全边界**（混淆非安全边界）。泄露爆炸半径 = 可轮换的第三方生图 API Key。
//! 若未来取得 Apple 证书，回迁 Data Protection Keychain 仅需换一个 [`SecretStore`] 实现。
//!
//! Key 本体永不进 DB / 日志 / 事件载荷：库里只存 `api_keys.keyring_account` 引用，
//! 日志只写 account 名。

// MemoryStore 供测试与后续命令测试使用；先落地。
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{AppError, AppResult};

const SERVICE: &str = "com.gendesk.app";

/// 主密钥字节数（XChaCha20-Poly1305）。
const KEY_LEN: usize = 32;
/// nonce 字节数（XChaCha20 扩展 nonce），密文文件前置存放。
const NONCE_LEN: usize = 24;

const KEY_FILE: &str = "secrets.key";
const ENC_FILE: &str = "secrets.enc";

pub trait SecretStore: Send + Sync {
    fn set(&self, account: &str, secret: &str) -> AppResult<()>;
    fn get(&self, account: &str) -> AppResult<Option<String>>;
    fn delete(&self, account: &str) -> AppResult<()>;
}

/// `secrets.enc` 解密后的明文结构。
#[derive(Debug, Default, Serialize, Deserialize)]
struct Vault {
    version: u32,
    keys: HashMap<String, String>,
}

/// 生产实现：本地加密文件（见模块头安全水位说明）。
pub struct FileStore {
    key_path: PathBuf,
    enc_path: PathBuf,
    /// 串行化「读—改—写」。single-instance 已禁双开，此锁只防同进程并发。
    lock: Mutex<()>,
}

impl FileStore {
    /// `dir` = app data 目录根。目录不存在时创建。
    pub fn new(dir: &Path) -> AppResult<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            key_path: dir.join(KEY_FILE),
            enc_path: dir.join(ENC_FILE),
            lock: Mutex::new(()),
        })
    }

    /// 读取主密钥；不存在则生成并落盘（0600）。
    ///
    /// 主密钥缺失但密文存在 → 密文已不可解，按损坏处理：留证后从空库重建。
    fn master_key(&self) -> AppResult<Key> {
        match std::fs::read(&self.key_path) {
            Ok(bytes) if bytes.len() == KEY_LEN => Ok(*Key::from_slice(&bytes)),
            Ok(_) => {
                tracing::warn!("密钥文件长度非法，已留证并重建（需在设置页重填 API Key）");
                self.quarantine(&self.key_path);
                self.quarantine(&self.enc_path);
                self.create_master_key()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if self.enc_path.exists() {
                    tracing::warn!("主密钥缺失但密文存在，已留证并重建（需在设置页重填 API Key）");
                    self.quarantine(&self.enc_path);
                }
                self.create_master_key()
            }
            Err(e) => Err(e.into()),
        }
    }

    fn create_master_key(&self) -> AppResult<Key> {
        let key = XChaCha20Poly1305::generate_key(&mut OsRng);
        write_private(&self.key_path, &key)?;
        Ok(key)
    }

    /// 把损坏 / 失效的文件改名留证（`<name>.bak-<秒级时间戳>`），失败只记 warn。
    /// 绝不写入任何密钥内容到日志。
    fn quarantine(&self, path: &Path) {
        if !path.exists() {
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut backup = path.as_os_str().to_os_string();
        backup.push(format!(".bak-{ts}"));
        if let Err(err) = std::fs::rename(path, PathBuf::from(backup)) {
            tracing::warn!(error = %err, file = ?path.file_name(), "留证损坏密钥文件失败");
        }
    }

    /// 解密并读出全量密钥表。密文缺失 = 空表；解密 / 解析失败 = 留证后重建为空表。
    fn load(&self, key: &Key) -> AppResult<HashMap<String, String>> {
        let raw = match std::fs::read(&self.enc_path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => return Err(e.into()),
        };
        if raw.len() <= NONCE_LEN {
            tracing::warn!("密钥密文长度非法，已留证并重建（需在设置页重填 API Key）");
            self.quarantine(&self.enc_path);
            return Ok(HashMap::new());
        }
        let (nonce, ct) = raw.split_at(NONCE_LEN);
        let cipher = XChaCha20Poly1305::new(key);
        let plain = match cipher.decrypt(XNonce::from_slice(nonce), ct) {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!("密钥密文解密失败，已留证并重建（需在设置页重填 API Key）");
                self.quarantine(&self.enc_path);
                return Ok(HashMap::new());
            }
        };
        match serde_json::from_slice::<Vault>(&plain) {
            Ok(v) => Ok(v.keys),
            Err(_) => {
                tracing::warn!("密钥明文结构非法，已留证并重建（需在设置页重填 API Key）");
                self.quarantine(&self.enc_path);
                Ok(HashMap::new())
            }
        }
    }

    /// 加密写回全量密钥表（每次写入重新随机 nonce；原子替换）。
    fn store(&self, key: &Key, keys: &HashMap<String, String>) -> AppResult<()> {
        let vault = Vault {
            version: 1,
            keys: keys.clone(),
        };
        let plain = serde_json::to_vec(&vault)?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = XChaCha20Poly1305::new(key)
            .encrypt(&nonce, plain.as_slice())
            .map_err(|_| AppError::Keyring("密钥加密失败".into()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        write_private(&self.enc_path, &out)
    }
}

impl SecretStore for FileStore {
    fn set(&self, account: &str, secret: &str) -> AppResult<()> {
        let _g = self.lock.lock().map_err(|_| poisoned())?;
        let key = self.master_key()?;
        let mut keys = self.load(&key)?;
        keys.insert(account.to_string(), secret.to_string());
        self.store(&key, &keys)
    }

    fn get(&self, account: &str) -> AppResult<Option<String>> {
        let _g = self.lock.lock().map_err(|_| poisoned())?;
        let key = self.master_key()?;
        Ok(self.load(&key)?.get(account).cloned())
    }

    fn delete(&self, account: &str) -> AppResult<()> {
        let _g = self.lock.lock().map_err(|_| poisoned())?;
        let key = self.master_key()?;
        let mut keys = self.load(&key)?;
        if keys.remove(account).is_none() {
            return Ok(());
        }
        self.store(&key, &keys)
    }
}

fn poisoned() -> AppError {
    AppError::Keyring("密钥存储锁已中毒".into())
}

/// 原子写：同目录临时文件 → fsync → rename 覆盖。
///
/// Unix 下临时文件以 0600 创建，rename 保留权限；Windows 无该语义，依赖用户
/// profile 目录 ACL（其他用户无法读取当前用户的 AppData）。
fn write_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    // 追加（而非替换）扩展名：secrets.key / secrets.enc 的临时文件必须互不相同。
    // 同进程内写入已被 Mutex 串行化，进程号只为避开他人残留。
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    // 目录项落盘，保证 rename 在崩溃后可见（best-effort，失败不阻断）。
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// 旧版实现：系统钥匙串（macOS Keychain / Windows 凭据管理器）。
///
/// **仅作迁移源**（[`migrate_from_keyring`]），不再用于生产读写；迁移期结束后
/// 与 `keyring` 依赖一并移除（见 docs/V2-backlog.md）。
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

/// 一次性迁移：系统钥匙串 → 本地加密文件。启动时调用，**幂等**（dest 已有即跳过）。
///
/// 账号名单取自 DB `api_keys.keyring_account`（库里只存引用）。
pub async fn migrate_from_keyring(pool: &SqlitePool, dest: &dyn SecretStore) -> AppResult<usize> {
    let accounts: Vec<String> =
        sqlx::query_scalar("SELECT keyring_account FROM api_keys ORDER BY id ASC")
            .fetch_all(pool)
            .await?;
    Ok(migrate_accounts(&accounts, &KeyringStore, dest))
}

/// 逐账号搬运，返回本次实际迁移条数。
///
/// - dest 已有 → 跳过，不动源（幂等）；
/// - 读源成功 → **先写 dest 落盘成功，再删源**（先删后写会在崩溃时丢密钥）；
/// - 读源失败 / 用户拒绝授权 → warn 后跳过且**不删源**，下次启动重试；
/// - 单条失败绝不中断整体（启动流程不能因迁移卡死）。
fn migrate_accounts(
    accounts: &[String],
    source: &dyn SecretStore,
    dest: &dyn SecretStore,
) -> usize {
    let mut moved = 0;
    for account in accounts {
        match dest.get(account) {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(account = %account, error = %err, "读取本地密钥文件失败，跳过迁移");
                continue;
            }
        }
        let secret = match source.get(account) {
            Ok(Some(s)) => s,
            // 源里本就没有（用户删过 / 从未配）：无可迁移，静默跳过。
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!(account = %account, error = %err, "读取钥匙串失败，保留源条目，下次启动重试");
                continue;
            }
        };
        if let Err(err) = dest.set(account, &secret) {
            tracing::warn!(account = %account, error = %err, "写入本地密钥文件失败，保留源条目");
            continue;
        }
        if let Err(err) = source.delete(account) {
            tracing::warn!(account = %account, error = %err, "清理钥匙串条目失败（密钥已迁移，可手动删除）");
        }
        moved += 1;
    }
    if moved > 0 {
        tracing::info!(count = moved, "API Key 已从系统钥匙串迁移到本地加密文件");
    }
    moved
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

#[cfg(test)]
// 测试内允许 unwrap/expect：构造与断言失败即测试失败，是期望行为。
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// 测试密钥一律用明显假值（禁 `sk-` 等仿真前缀，避免 secret scanning 误报）。
    const FAKE: &str = "test-key-0001";

    fn store(dir: &Path) -> FileStore {
        FileStore::new(dir).unwrap()
    }

    #[test]
    fn set_get_delete_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());

        assert_eq!(s.get("acct-1").unwrap(), None);
        s.set("acct-1", FAKE).unwrap();
        s.set("acct-2", "test-key-0002").unwrap();
        assert_eq!(s.get("acct-1").unwrap().as_deref(), Some(FAKE));
        assert_eq!(s.get("acct-2").unwrap().as_deref(), Some("test-key-0002"));

        s.delete("acct-1").unwrap();
        assert_eq!(s.get("acct-1").unwrap(), None);
        // 删一条不影响另一条。
        assert_eq!(s.get("acct-2").unwrap().as_deref(), Some("test-key-0002"));
        // 删不存在的 account 返回 Ok。
        s.delete("acct-1").unwrap();
        s.delete("never").unwrap();
    }

    /// 重开实例仍可读 —— 密钥真正落盘，不是进程内缓存。
    #[test]
    fn persists_across_instances() {
        let tmp = tempfile::tempdir().unwrap();
        store(tmp.path()).set("acct-1", FAKE).unwrap();

        let reopened = store(tmp.path());
        assert_eq!(reopened.get("acct-1").unwrap().as_deref(), Some(FAKE));
    }

    /// 磁盘上不得出现密钥明文（文件层加密的全部意义）。
    #[test]
    fn ciphertext_contains_no_plaintext() {
        let tmp = tempfile::tempdir().unwrap();
        store(tmp.path()).set("acct-1", FAKE).unwrap();

        let raw = std::fs::read(tmp.path().join(ENC_FILE)).unwrap();
        assert!(!raw.windows(FAKE.len()).any(|w| w == FAKE.as_bytes()));
    }

    /// 密文损坏 → 留证 `.bak-*` + 从空库重建 + 后续 set 可用（自愈，不阻断启动）。
    #[test]
    fn corrupted_ciphertext_self_heals_with_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.set("acct-1", FAKE).unwrap();

        std::fs::write(
            tmp.path().join(ENC_FILE),
            b"not a valid ciphertext at all!!!!",
        )
        .unwrap();

        assert_eq!(s.get("acct-1").unwrap(), None, "损坏后读出空，不报错");
        s.set("acct-1", "test-key-0003").unwrap();
        assert_eq!(s.get("acct-1").unwrap().as_deref(), Some("test-key-0003"));

        let backups = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
            .count();
        assert!(backups >= 1, "损坏文件应改名留证");
    }

    /// 主密钥丢失（密文还在）→ 密文已不可解，同样留证重建而非报错。
    #[test]
    fn missing_master_key_rebuilds_with_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.set("acct-1", FAKE).unwrap();
        std::fs::remove_file(tmp.path().join(KEY_FILE)).unwrap();

        assert_eq!(s.get("acct-1").unwrap(), None);
        assert!(tmp.path().join(KEY_FILE).exists(), "主密钥应重新生成");
        s.set("acct-1", FAKE).unwrap();
        assert_eq!(s.get("acct-1").unwrap().as_deref(), Some(FAKE));
    }

    #[cfg(unix)]
    #[test]
    fn files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        store(tmp.path()).set("acct-1", FAKE).unwrap();

        for name in [KEY_FILE, ENC_FILE] {
            let mode = std::fs::metadata(tmp.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{name} 权限应为 0600，实为 {mode:o}");
        }
    }

    /// 读源恒失败的 store，用于验证「失败条目不删源、其余照迁」。
    struct FailingSource {
        inner: MemoryStore,
        fail_on: &'static str,
    }

    impl SecretStore for FailingSource {
        fn set(&self, account: &str, secret: &str) -> AppResult<()> {
            self.inner.set(account, secret)
        }
        fn get(&self, account: &str) -> AppResult<Option<String>> {
            if account == self.fail_on {
                return Err(AppError::Keyring("用户拒绝授权".into()));
            }
            self.inner.get(account)
        }
        fn delete(&self, account: &str) -> AppResult<()> {
            self.inner.delete(account)
        }
    }

    #[test]
    fn migration_moves_all_and_clears_source() {
        let src = MemoryStore::default();
        src.set("acct-1", FAKE).unwrap();
        src.set("acct-2", "test-key-0002").unwrap();
        let dest = MemoryStore::default();

        let accounts = vec!["acct-1".to_string(), "acct-2".to_string()];
        assert_eq!(migrate_accounts(&accounts, &src, &dest), 2);

        assert_eq!(dest.get("acct-1").unwrap().as_deref(), Some(FAKE));
        assert_eq!(
            dest.get("acct-2").unwrap().as_deref(),
            Some("test-key-0002")
        );
        assert_eq!(src.get("acct-1").unwrap(), None, "源应被清空");
        assert_eq!(src.get("acct-2").unwrap(), None, "源应被清空");
    }

    /// 幂等：dest 已有的条目跳过，且**不动源**（不会误删用户手填后残留的源条目）。
    #[test]
    fn migration_is_idempotent_and_skips_existing() {
        let src = MemoryStore::default();
        src.set("acct-1", "test-key-old").unwrap();
        let dest = MemoryStore::default();
        dest.set("acct-1", "test-key-new").unwrap();

        let accounts = vec!["acct-1".to_string()];
        assert_eq!(migrate_accounts(&accounts, &src, &dest), 0);
        assert_eq!(
            dest.get("acct-1").unwrap().as_deref(),
            Some("test-key-new"),
            "dest 已有值不得被覆盖"
        );
        assert_eq!(
            src.get("acct-1").unwrap().as_deref(),
            Some("test-key-old"),
            "跳过的条目不动源"
        );
    }

    /// 单条读源失败（如用户点「拒绝」）：源保留供下次重试，其余条目照迁，整体不中断。
    #[test]
    fn migration_keeps_source_on_read_failure() {
        let inner = MemoryStore::default();
        inner.set("acct-1", FAKE).unwrap();
        inner.set("acct-2", "test-key-0002").unwrap();
        let src = FailingSource {
            inner,
            fail_on: "acct-1",
        };
        let dest = MemoryStore::default();

        let accounts = vec!["acct-1".to_string(), "acct-2".to_string()];
        assert_eq!(migrate_accounts(&accounts, &src, &dest), 1);

        assert_eq!(dest.get("acct-1").unwrap(), None, "失败条目未迁");
        assert_eq!(
            src.inner.get("acct-1").unwrap().as_deref(),
            Some(FAKE),
            "失败条目必须保留源，下次启动重试"
        );
        assert_eq!(
            dest.get("acct-2").unwrap().as_deref(),
            Some("test-key-0002")
        );
        assert_eq!(src.inner.get("acct-2").unwrap(), None);
    }

    #[test]
    fn mask_hides_all_but_last_four() {
        assert_eq!(mask("test-key-0001"), "****0001");
        assert_eq!(mask("ab"), "****");
        assert_eq!(mask("abcd"), "****");
    }
}
