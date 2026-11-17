//! 网盘凭据本机加密存储（W13 D4–D6 / W14 §9）。
//!
//! - 文件：`<app_data_dir>/credentials.json`（Windows 即 `%APPDATA%/VBox/credentials.json`）。
//! - Windows：每条凭据用 DPAPI `CryptProtectData` 加密（绑定当前 Windows 用户），
//!   文件里只有密文；非 Windows 平台降级为受限权限文件（W13 §6 明确允许）。
//! - 安全约束：cookie/token 明文只在内存与 DPAPI 明文块中出现，**不写日志、不返回前端**；
//!   前端只拿 [`AccountView`] 摘要（账号掩码 + 状态 + 更新时间）。
//! - schema：`{"version":1,"entries":[{"payload":"<base64(加密 Credential JSON)>"}]}`，
//!   后续加 account_id 主键/迁移时用 version 字段升级。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use vbox_core::pan::login::{self, AccountView, Credential};

/// 文件格式版本。
const FORMAT_VERSION: u32 = 1;
/// 载荷类型：Windows DPAPI 密文。
const PAYLOAD_DPAPI: u8 = 0x01;
/// 载荷类型：非 Windows 降级明文。
const PAYLOAD_PLAIN: u8 = 0x00;

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<StoredEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredEntry {
    /// base64(载荷)，载荷首字节标识加密方式。
    payload: String,
}

/// 凭据存储：文件读写串行化，内存 overlay 由 `vbox_core::pan::login` 持有。
pub struct PanCredentialStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl PanCredentialStore {
    /// `data_dir` 为 Tauri 的 `app_data_dir`。
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("credentials.json"),
            lock: Mutex::new(()),
        }
    }

    /// 凭据文件路径（诊断与测试用；内容为密文）。
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 从磁盘恢复所有凭据到内存 overlay（每个 provider 取 `updated_at` 最新的一份）。
    ///
    /// 返回全部账号摘要（含过期项），供账号管理页首屏使用。
    pub fn restore_overlay(&self) -> Vec<AccountView> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        match self.read_locked() {
            Ok(creds) => {
                let mut latest: std::collections::BTreeMap<String, &Credential> =
                    std::collections::BTreeMap::new();
                for cred in &creds {
                    let entry = latest.entry(cred.provider.clone()).or_insert(cred);
                    if cred.updated_at >= entry.updated_at {
                        *entry = cred;
                    }
                }
                for cred in latest.values() {
                    if let Err(err) = login::install_credential((*cred).clone()) {
                        eprintln!("[vbox] 恢复网盘凭据失败（{}）: {err}", cred.provider);
                    }
                }
                views_of(&creds)
            }
            Err(err) => {
                eprintln!("[vbox] 读取网盘凭据失败: {err}");
                Vec::new()
            }
        }
    }

    /// 列出所有已保存账号摘要（账号管理页）。
    pub fn list(&self) -> Result<Vec<AccountView>, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(views_of(&self.read_locked()?))
    }

    /// 保存/更新一份凭据：按 `account_id` 覆盖旧记录，同步刷新内存 overlay。
    pub fn upsert(&self, cred: Credential) -> Result<AccountView, String> {
        let cred = login::normalize_credential(cred).map_err(|e| e.to_string())?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut creds = self.read_locked()?;
        let account_id = cred.effective_account_id();
        creds.retain(|c| c.effective_account_id() != account_id);
        creds.push(cred.clone());
        self.write_locked(&creds)?;
        login::install_credential(cred.clone()).map_err(|e| e.to_string())?;
        Ok(AccountView::from_credential(&cred))
    }

    /// 按 account_id 删除账号；若删除的是该 provider 的活跃账号，同时更新 overlay。
    pub fn remove(&self, account_id: &str) -> Result<bool, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut creds = self.read_locked()?;
        let removed: Vec<Credential> = creds
            .iter()
            .filter(|c| c.effective_account_id() == account_id)
            .cloned()
            .collect();
        if removed.is_empty() {
            return Ok(false);
        }
        creds.retain(|c| c.effective_account_id() != account_id);
        self.write_locked(&creds)?;
        for cred in removed {
            let active_is_removed = login::active_credential(&cred.provider)
                .map(|a| a.effective_account_id() == account_id)
                .unwrap_or(false);
            if active_is_removed {
                login::remove_credential(&cred.provider);
                // 该 provider 还有其它账号时，把最新的一份放回 overlay。
                if let Some(next) = creds
                    .iter()
                    .filter(|c| c.provider == cred.provider)
                    .max_by_key(|c| c.updated_at)
                {
                    let _ = login::install_credential(next.clone());
                }
            }
        }
        Ok(true)
    }

    /// 退出某个 provider 的全部账号（W13 `credentials_clear(provider)`）。
    pub fn remove_provider(&self, provider: &str) -> Result<usize, String> {
        let provider = login::normalize_provider(provider)
            .ok_or_else(|| format!("未知网盘 provider：{provider}"))?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut creds = self.read_locked()?;
        let before = creds.len();
        creds.retain(|c| c.provider != provider);
        let removed = before - creds.len();
        if removed > 0 {
            self.write_locked(&creds)?;
            login::remove_credential(&provider);
        }
        Ok(removed)
    }

    /// 当前活跃凭据是否可用（未过期且有 cookie/token）。
    pub fn verify(&self, provider: &str) -> bool {
        login::overlay_active(provider).is_some()
    }

    /// 按 `account_id` 取一份完整凭据（只读云盘管理用；调用方不得外传 cookie/token）。
    pub fn credential(&self, account_id: &str) -> Result<Option<Credential>, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let creds = self.read_locked()?;
        Ok(creds
            .into_iter()
            .find(|cred| cred.effective_account_id() == account_id))
    }

    /// 读取并解密全部凭据（不含锁，调用方已持有）。
    fn read_locked(&self) -> Result<Vec<Credential>, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(format!("读取网盘凭据文件失败: {err}")),
        };
        let file: StoreFile =
            serde_json::from_str(&text).map_err(|e| format!("网盘凭据文件格式错误: {e}"))?;
        let mut out = Vec::with_capacity(file.entries.len());
        for (idx, entry) in file.entries.iter().enumerate() {
            let raw = STANDARD
                .decode(entry.payload.as_bytes())
                .map_err(|e| format!("第 {idx} 条凭据 base64 解码失败: {e}"))?;
            let plain = unprotect(&raw).map_err(|e| format!("第 {idx} 条凭据解密失败: {e}"))?;
            let cred: Credential = serde_json::from_slice(&plain)
                .map_err(|e| format!("第 {idx} 条凭据 JSON 解析失败: {e}"))?;
            out.push(cred);
        }
        Ok(out)
    }

    /// 加密并覆盖写文件（调用方已持有锁）。
    fn write_locked(&self, creds: &[Credential]) -> Result<(), String> {
        let mut file = StoreFile {
            version: FORMAT_VERSION,
            entries: Vec::with_capacity(creds.len()),
        };
        for cred in creds {
            let plain = serde_json::to_vec(cred).map_err(|e| format!("凭据序列化失败: {e}"))?;
            let payload = protect(&plain)?;
            file.entries.push(StoredEntry {
                payload: STANDARD.encode(payload),
            });
        }
        let text = serde_json::to_string_pretty(&file)
            .map_err(|e| format!("凭据文件序列化失败: {e}"))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建凭据目录失败（{}）: {e}", parent.display()))?;
        }
        std::fs::write(&self.path, text).map_err(|e| format!("写入网盘凭据失败: {e}"))?;
        restrict_permissions(&self.path);
        Ok(())
    }
}

/// 凭据 → 摘要列表（provider 升序，同 provider 更新时间降序）。
fn views_of(creds: &[Credential]) -> Vec<AccountView> {
    let mut views: Vec<AccountView> = creds.iter().map(AccountView::from_credential).collect();
    views.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then(b.updated_at.cmp(&a.updated_at))
    });
    views
}

/// 非 Windows 平台限制文件权限（仅当前用户可读写）。
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// 加密载荷：Windows 走 DPAPI，其它平台标记为明文降级。
fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(plain.len() + 1);
    #[cfg(windows)]
    {
        out.push(PAYLOAD_DPAPI);
        out.extend(dpapi::protect(plain)?);
    }
    #[cfg(not(windows))]
    {
        out.push(PAYLOAD_PLAIN);
        out.extend_from_slice(plain);
    }
    Ok(out)
}

/// 解密载荷：按首字节选择 DPAPI / 降级明文。
fn unprotect(payload: &[u8]) -> Result<Vec<u8>, String> {
    let (tag, body) = payload
        .split_first()
        .ok_or_else(|| "凭据载荷为空".to_string())?;
    match *tag {
        PAYLOAD_DPAPI => {
            #[cfg(windows)]
            {
                dpapi::unprotect(body)
            }
            #[cfg(not(windows))]
            {
                Err("该凭据由 Windows DPAPI 加密，本机无法解密（请重新登录）".to_string())
            }
        }
        PAYLOAD_PLAIN => Ok(body.to_vec()),
        other => Err(format!("未知凭据载荷格式: 0x{other:02x}")),
    }
}

/// Windows DPAPI（`CryptProtectData` / `CryptUnprotectData`）。
///
/// 直接声明 crypt32/kernel32 符号，避免为两个 API 引入整个 windows crate。
#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    fn last_error(context: &str) -> String {
        format!(
            "{context}: {}",
            std::io::Error::last_os_error()
        )
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        let input = DataBlob {
            cb_data: plain.len() as u32,
            pb_data: plain.as_ptr() as *mut u8,
        };
        let mut output = DataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        // SAFETY: 输入/输出 blob 均指向本栈帧内的有效数据；DPAPI 只在调用期间读写它们。
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(last_error("DPAPI 加密失败"));
        }
        let data = if output.pb_data.is_null() || output.cb_data == 0 {
            Vec::new()
        } else {
            // SAFETY: 成功时 DPAPI 返回 `output.cb_data` 字节的 LocalAlloc 缓冲。
            unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() }
        };
        // SAFETY: 该缓冲由 CryptProtectData 分配，必须用 LocalFree 释放。
        unsafe {
            LocalFree(output.pb_data as *mut c_void);
        }
        Ok(data)
    }

    pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>, String> {
        let input = DataBlob {
            cb_data: cipher.len() as u32,
            pb_data: cipher.as_ptr() as *mut u8,
        };
        let mut output = DataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        // SAFETY: 同 protect。
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(last_error("DPAPI 解密失败"));
        }
        let data = if output.pb_data.is_null() || output.cb_data == 0 {
            Vec::new()
        } else {
            // SAFETY: 成功时 DPAPI 返回 `output.cb_data` 字节的 LocalAlloc 缓冲。
            unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() }
        };
        // SAFETY: 同 protect。
        unsafe {
            LocalFree(output.pb_data as *mut c_void);
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vbox_core::pan::login::{P115, QUARK};

    /// overlay 是进程级全局状态，串行执行本模块的落盘/恢复测试。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 测试用临时目录：放在 <crate>/target 下，避免系统 Temp 在某些环境下不可写。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("target")
            .join(format!(
                "pan-cred-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 清掉本模块用到的 provider 的全局 overlay，避免测试间相互影响。
    fn reset_overlay(providers: &[&str]) {
        for provider in providers {
            login::remove_credential(provider);
        }
    }

    fn sample(provider: &str, cookie: &str) -> Credential {
        let mut cred = Credential::new(provider);
        cred.cookie = Some(cookie.to_string());
        cred.display_name = Some("测试账号".to_string());
        cred.masked_id = Some("u***88".to_string());
        cred.account_id = format!("{provider}:tester");
        cred
    }

    #[test]
    fn store_roundtrip_and_overlay() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_overlay(&[P115, QUARK]);
        let dir = temp_dir("roundtrip");
        let store = PanCredentialStore::new(&dir);
        let view = store.upsert(sample(P115, "UID=1; CID=2")).unwrap();
        assert_eq!(view.provider, P115);
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(store.verify(P115));
        assert!(store.verify(QUARK) == false);
        // 模拟重启：清 overlay 后从磁盘恢复。
        login::remove_credential(P115);
        let restored = store.restore_overlay();
        assert_eq!(restored.len(), 1);
        assert!(login::overlay_active(P115).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_replaces_same_account_and_remove_clears_overlay() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_overlay(&[P115, QUARK]);
        let dir = temp_dir("replace");
        let store = PanCredentialStore::new(&dir);
        store.upsert(sample(P115, "UID=1")).unwrap();
        let mut updated = sample(P115, "UID=9");
        updated.updated_at = 10;
        store.upsert(updated).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].account_id, "p115:tester");
        assert!(store.remove("p115:tester").unwrap());
        assert!(login::overlay_active(P115).is_none());
        assert!(store.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_contains_no_plaintext_cookie() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_overlay(&[P115, QUARK]);
        let dir = temp_dir("ciphertext");
        let store = PanCredentialStore::new(&dir);
        store.upsert(sample(QUARK, "__pus=secret_cookie_value")).unwrap();
        let text = std::fs::read_to_string(store.path()).unwrap();
        assert!(
            !text.contains("secret_cookie_value"),
            "凭据文件不应出现明文 cookie: {text}"
        );
        assert!(text.contains("entries"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip() {
        let cipher = dpapi::protect(b"secret payload").unwrap();
        assert_ne!(cipher, b"secret payload");
        assert_eq!(dpapi::unprotect(&cipher).unwrap(), b"secret payload");
    }
}