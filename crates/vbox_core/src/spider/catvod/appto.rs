//! `appto`（fantaiying）App 框架协议引擎（第 123 轮 W10.1）。
//!
//! ## 来源与证据
//!
//! `csp_AppMao` / `csp_AppSK` / `csp_AppLd` / `csp_MLYS` 都出自同一 jar
//! `69dadb9a-Yoursmile.jar`，ext 同为 `Fbj…` 的 base64 串，协议前缀
//! `/addons/apptov4/app.php/v1/`、`/appto/v1/`。jadx 对 `AppMao` 只能还原 2002 行
//! （37 处 skip，`init`/`formUpload` 被降级为 `UnsupportedOperationException`），
//! 第 123 轮改用「dex2jar 2.4 → Vineflower 1.10.1 单类」链得到 4373 行、0 失败的
//! `AppMao.java`，再用短数组 XOR 解码器解出 142/142 条字符串。
//!
//! ## ext 解码（已对 5 个真实配置验证 5/5）
//!
//! `base64(ext)` → **RC4（key = `fantaiying666`）** → **每字节 -1** → UTF-8 JSON：
//!
//! - 萌米：`domain=http://lmapp.org/appto.json`、`key=QBcc…`、`isV4=true`；
//! - 星奇/繁星：`domain=…/accredits/25.json`、`configV1=true`、`host=http://cms1.xingqisp.com`；
//! - 南坊：`domain=…/accredits/57.json`、`configV1=true`；
//! - 萌米App：`host=http://app.mervod.com:9527`、`isV4=true`。
//!
//! ## 配置响应解密
//!
//! `domain` 指向的 JSON 为 `{"ENCRYPTION":0|1,"data":…}`：
//! `ENCRYPTION==1` 时 `data` 即明文 JSON；否则 `data` 是
//! **AES-256-CBC（PKCS5Padding）** 的 base64 密文，key/iv 取自 ext 的 `key`/`iv`
//! （ext 里 `key` 为 32 字符、`iv` 为 16 字符）。
//!
//! ## 本模块边界
//!
//! 只做「ext 解码 + 配置解密 + 端点路径」三件事，供后续 `appmao` 家族直接使用。
//! 上游五个配置地址在 2026-11 已全部失效（`lmapp.org`/`app.mervod.com` fetch
//! failed，cos 桶返回 `NoSuchBucket`），因此本模块的测试全部使用离线向量。

use aes::{Aes128, Aes192, Aes256};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use serde_json::Value;

/// ext 的 RC4 密钥（由 `oS` 类的短数组 `decode(arr, 239, 13, 1733)` 解出）。
pub const RC4_KEY: &str = "fantaiying666";
/// 配置请求的 UA（解码字符串 `Dart/2.19 (dart:io)`）。
pub const UA: &str = "Dart/2.19 (dart:io)";

/// v1 家族的配置端点。
pub const CONFIG_V1: &str = "/appto/v1/config/get?p=android";
/// v2（`configV2:true`）家族的配置端点。
pub const CONFIG_V2: &str = "/appto/v2/config/get?p=android";
/// v1 家族的业务前缀。
pub const API_V1: &str = "/appto/v1/";
/// v4（`isV4:true`）家族的业务前缀。
pub const API_V4: &str = "/addons/apptov4/app.php/v1/";

/// 业务端点（拼在 `api_prefix()` 之后，均不带前导 `/`）。
pub const EP_HOME_CATE: &str = "home/cateData";
pub const EP_GET_LISTS: &str = "vod/getLists";
pub const EP_GET_VOD: &str = "vod/getVod";
pub const EP_SEARCH: &str = "vod/getVodSearch";
pub const EP_PARSING: &str = "parsing/proxy";

/// 一个站点的 ext 解码结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApptoExt {
    /// 配置 JSON 地址（`domain`）。
    pub domain: Option<String>,
    /// 业务基址（`host`，可能是空串）。
    pub host: Option<String>,
    /// AES key（32 字符）。
    pub key: Option<String>,
    /// AES iv（16 字符）。
    pub iv: Option<String>,
    /// 设备 token（`token`）。
    pub token: Option<String>,
    /// 设备指纹（`__ic`）。
    pub ic: Option<String>,
    /// `isV4`：使用 `/addons/apptov4/app.php/v1/` 前缀。
    pub is_v4: bool,
    /// 旧版 `configV1` 标记。
    pub config_v1: bool,
    /// `configV2`：配置端点走 `/appto/v2/config/get`。
    pub config_v2: bool,
}

impl ApptoExt {
    /// 业务前缀。
    pub fn api_prefix(&self) -> &'static str {
        if self.is_v4 { API_V4 } else { API_V1 }
    }

    /// 配置端点。
    pub fn config_path(&self) -> &'static str {
        if self.config_v2 { CONFIG_V2 } else { CONFIG_V1 }
    }

    /// 可用的业务基址（`host` 非空时）。
    pub fn base_host(&self) -> Option<&str> {
        self.host.as_deref().filter(|h| !h.trim().is_empty())
    }
}

/// RC4（KSA + PRGA）。
pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = core::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let mut out = Vec::with_capacity(data.len());
    let (mut i, mut j) = (0u8, 0u8);
    for &b in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[(s[i as usize] as usize + s[j as usize] as usize) & 0xff];
        out.push(b ^ k);
    }
    out
}

/// ext 的 `-1` 逆变换。
fn unshift_minus_one(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|b| b.wrapping_sub(1)).collect()
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s.trim()).map_err(|e| format!("ext 不是合法 base64: {e}"))
}

/// 解码站点 ext。
///
/// `base64` → RC4(`fantaiying666`) → 每字节 -1 → JSON → `ApptoExt`。
pub fn decode_ext(ext: &str) -> Result<ApptoExt, String> {
    let ext = ext.trim();
    if ext.is_empty() {
        return Err("ext 为空".to_string());
    }
    let raw = base64_decode(ext)?;
    let plain = unshift_minus_one(&rc4(RC4_KEY.as_bytes(), &raw));
    let text = String::from_utf8(plain).map_err(|e| format!("ext 解码后不是 UTF-8: {e}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("ext 解码后不是 JSON: {e}"))?;
    let obj = value.as_object().ok_or_else(|| "ext 解码后不是 JSON 对象".to_string())?;
    let s = |k: &str| -> Option<String> {
        obj.get(k)
            .and_then(|v| v.as_str())
            .map(|v| v.trim().to_string())
    };
    let flag = |k: &str| -> bool {
        match obj.get(k) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0) == 1,
            Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
            _ => false,
        }
    };
    Ok(ApptoExt {
        domain: s("domain"),
        host: s("host"),
        key: s("key"),
        iv: s("iv"),
        token: s("token"),
        ic: s("__ic"),
        is_v4: flag("isV4"),
        config_v1: flag("configV1"),
        config_v2: flag("configV2"),
    })
}

fn aes256_cbc_decrypt(key: &str, iv: &str, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let (kb, ib) = (key.as_bytes(), iv.as_bytes());
    if kb.len() != 32 {
        return Err(format!("AES key 需要 32 字节，实际 {}", kb.len()));
    }
    if ib.len() != 16 {
        return Err(format!("AES iv 需要 16 字节，实际 {}", ib.len()));
    }
    let cipher = cbc::Decryptor::<Aes256>::new_from_slices(kb, ib)
        .map_err(|e| format!("AES 初始化失败: {e}"))?;
    let mut buf = ciphertext.to_vec();
    let plain = cipher
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| format!("AES-CBC 解密失败: {e}"))?;
    Ok(plain.to_vec())
}

fn data_value(value: Value) -> Result<Value, String> {
    match value {
        Value::String(text) => serde_json::from_str(&text)
            .map_err(|e| format!("配置 data 不是 JSON: {e}")),
        Value::Object(_) | Value::Array(_) => Ok(value),
        _ => Err("配置 data 类型不支持".to_string()),
    }
}

/// 解密 `domain` 返回的配置信封。
///
/// `{"ENCRYPTION":1,"data":…}` → `data` 即明文 JSON；
/// 其余情况 → `data` 为 AES-256-CBC/PKCS5 的 base64 密文，key/iv 取 ext。
pub fn decrypt_config(ext: &ApptoExt, body: &str) -> Result<Value, String> {
    let envelope: Value =
        serde_json::from_str(body).map_err(|e| format!("配置响应不是 JSON: {e}"))?;
    let obj = envelope
        .as_object()
        .ok_or_else(|| "配置响应不是 JSON 对象".to_string())?;
    let encrypted = match obj.get("ENCRYPTION") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) == 1,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    };
    let data = obj.get("data").cloned().ok_or_else(|| "配置响应缺少 data".to_string())?;
    if encrypted {
        return data_value(data);
    }
    let text = data
        .as_str()
        .ok_or_else(|| "加密的配置 data 必须是 base64 字符串".to_string())?;
    let key = ext.key.as_deref().ok_or_else(|| "ext 缺少 key，无法解密配置".to_string())?;
    let iv = ext.iv.as_deref().ok_or_else(|| "ext 缺少 iv，无法解密配置".to_string())?;
    let ciphertext = base64_decode(text)?;
    let plain = aes256_cbc_decrypt(key, iv, &ciphertext)?;
    let text = String::from_utf8(plain).map_err(|e| format!("AES 解密后不是 UTF-8: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("AES 解密后不是 JSON: {e}"))
}


/// 通用 ext 解码：返回原始 JSON（供非 appto 字段结构的应用框架家族使用，如 AppLd）。
pub fn decode_ext_value(ext: &str) -> Result<Value, String> {
    let ext = ext.trim();
    if ext.is_empty() {
        return Err("ext 为空".to_string());
    }
    let raw = base64_decode(ext)?;
    let plain = unshift_minus_one(&rc4(RC4_KEY.as_bytes(), &raw));
    let text = String::from_utf8(plain).map_err(|e| format!("ext 解码后不是 UTF-8: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("ext 解码后不是 JSON: {e}"))
}

/// AES-CBC/PKCS7 解密：key 支持 16/24/32 字节（AES-128/192/256）。
pub fn aes_cbc_decrypt(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let mut buf = ciphertext.to_vec();
    let out = match key.len() {
        16 => cbc::Decryptor::<Aes128>::new_from_slices(key, iv)
            .map_err(|e| e.to_string())?
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|e| e.to_string())?
            .to_vec(),
        24 => cbc::Decryptor::<Aes192>::new_from_slices(key, iv)
            .map_err(|e| e.to_string())?
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|e| e.to_string())?
            .to_vec(),
        32 => cbc::Decryptor::<Aes256>::new_from_slices(key, iv)
            .map_err(|e| e.to_string())?
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|e| e.to_string())?
            .to_vec(),
        n => return Err(format!("AES key 长度 {n} 不支持")),
    };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXT_MENGMI: &str = "FbjDcUxPqpfNr0QF4QvEpcoxMMdAA5wQTClJt+Hxgw4VXJLfrTSRjQjA6r70U2ONX3iG4lQWk1cgXQMSUZpbUzIYJ6iQs3+gcUVDp1lz6rcrGqDtCW4hfAf9JSR025m0+TGWMsZkVg==";
    const EXT_MENGMI_APP: &str = "FbjKZ1gO+9u81wMIxzHg+7x1Oep4CIFuZAlCy8a4qCgjGLacnxDo6wiBr66jP3X8J2mRx31u1XgkTTVrENVjfhQKUN2Yi12XNWQSlmVZxLtiSaSiZG0xf1Hw";
    const EXT_XINGQI: &str = "FbjDcUxPqpfNr0QF4QvE6sExbd4UXJxJXzdL462ywU1XScGa5G6Hj0/c+Ou1GW6rdX6N2XIhnD46QzIsRoZ8bk4fG4OYi0iCaWwRj2ddkacwFqHtLjQhalHqIy0+kpiTv2eOfJYxTshgrxcJ+g3lEHx7ZLC9kB1TCfZUSHwqHB3tt6V/1OhRENIOZNRFfXVBEFd7jQg+J06kjCAF7z1Bt8hRvMjC2VcbQXsEz8MCZDgu06C9/wirWcTa/wWJsfT+Z7fXaDs+dIWMYiwf6td5CWTx6LfL6eTuiMhs2KU5Byw9F+4Y/AHBg4r+LEe+Hm4EM2d8X3ca5zHSj+kjmpXOiGDo4TalFqgTq67eggmthCbV2d131SKc03V+5gruudQQk/Tb9e4lg6SQhA9/5kWOOM/LRmkqIxSt6d0wpu7yKKh65enwKbgasc93/HnGr6W4LuZjf6eYnS2GgRzWRE0fT5lYNmxnWeRnYuLhoZbDqslAfJzg";

    #[test]
    fn rc4_matches_rfc6229_vector() {
        assert_eq!(hex::encode(rc4(b"Key", b"Plaintext")), "bbf316e8d940af0ad3");
    }

    #[test]
    fn decodes_mengmi_ext() {
        let ext = decode_ext(EXT_MENGMI).expect("mengmi ext");
        assert_eq!(ext.domain.as_deref(), Some("http://lmapp.org/appto.json"));
        assert_eq!(ext.key.as_deref(), Some("QBccRNTdEscBYykPWRbKD7YREjN6AJYJ"));
        assert!(ext.is_v4);
        assert!(!ext.config_v1 && !ext.config_v2);
        assert_eq!(ext.base_host(), None);
        assert_eq!(ext.api_prefix(), API_V4);
        assert_eq!(ext.config_path(), CONFIG_V1);
    }

    #[test]
    fn decodes_mengmi_app_ext() {
        let ext = decode_ext(EXT_MENGMI_APP).expect("mengmi app ext");
        assert_eq!(ext.base_host(), Some("http://app.mervod.com:9527"));
        assert!(ext.is_v4);
        assert_eq!(ext.domain, None);
        assert_eq!(ext.api_prefix(), API_V4);
    }

    #[test]
    fn decodes_xingqi_ext_flags() {
        let ext = decode_ext(EXT_XINGQI).expect("xingqi ext");
        assert!(ext.config_v1);
        assert!(!ext.is_v4);
        assert_eq!(ext.base_host(), Some("http://cms1.xingqisp.com"));
        let ic = ext.ic.expect("ic");
        assert!(ic.starts_with("N2SuKk"), "ic 前缀: {ic}");
    }

    #[test]
    fn rejects_invalid_ext() {
        assert!(decode_ext("").is_err());
        assert!(decode_ext("!!!not-base64!!!").is_err());
        // 合法 base64 但不是该框架的 ext（RC4/-1 后不是 JSON）
        assert!(decode_ext("aGVsbG8gd29ybGQ=").is_err());
    }

    fn make_envelope(plaintext: &str, key: &str, iv: &str) -> (String, ApptoExt) {
        use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        let enc = cbc::Encryptor::<Aes256>::new_from_slices(key.as_bytes(), iv.as_bytes())
            .expect("encryptor");
        let mut buf = vec![0u8; plaintext.len() + 16];
        buf[..plaintext.len()].copy_from_slice(plaintext.as_bytes());
        let ct = enc
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
            .expect("encrypt")
            .to_vec();
        let body = format!(
            "{{\"ENCRYPTION\":0,\"data\":\"{}\"}}",
            B64.encode(&ct)
        );
        let ext = ApptoExt {
            key: Some(key.to_string()),
            iv: Some(iv.to_string()),
            ..ApptoExt::default()
        };
        (body, ext)
    }

    #[test]
    fn decrypts_aes256_config_envelope() {
        let key = "0123456789abcdef0123456789abcdef";
        let iv = "0123456789abcdef";
        let plaintext = r#"{"host":"http://api.example.com","types":[{"type_id":"1","type_name":"电影"}]}"#;
        let (body, ext) = make_envelope(plaintext, key, iv);
        let value = decrypt_config(&ext, &body).expect("decrypt");
        assert_eq!(
            value["host"].as_str(),
            Some("http://api.example.com")
        );
        assert_eq!(value["types"][0]["type_name"].as_str(), Some("电影"));
    }

    #[test]
    fn plaintext_envelope_is_returned() {
        let ext = ApptoExt::default();
        let body = r#"{"ENCRYPTION":1,"data":{"host":"http://h","parsing":[]}}"#;
        let value = decrypt_config(&ext, body).expect("plain");
        assert_eq!(value["host"].as_str(), Some("http://h"));
        let body2 = r#"{"ENCRYPTION":1,"data":"{\"host\":\"http://str\"}"}"#;
        assert_eq!(
            decrypt_config(&ext, body2).expect("plain str")["host"].as_str(),
            Some("http://str")
        );
    }

    #[test]
    fn encrypted_envelope_requires_key_and_iv() {
        let ext = ApptoExt::default();
        let body = r#"{"ENCRYPTION":0,"data":"AAAA"}"#;
        let err = decrypt_config(&ext, body).unwrap_err();
        assert!(err.contains("key"), "err={err}");
    }

    #[test]
    fn endpoint_helpers_cover_both_families() {
        let v1 = ApptoExt::default();
        assert_eq!(v1.api_prefix(), API_V1);
        assert_eq!(v1.config_path(), CONFIG_V1);
        let v2 = ApptoExt {
            config_v2: true,
            is_v4: false,
            ..ApptoExt::default()
        };
        assert_eq!(v2.config_path(), CONFIG_V2);
        assert_eq!(format!("{}{}", API_V1, EP_GET_LISTS), "/appto/v1/vod/getLists");
        assert_eq!(
            format!("{}{}", API_V4, EP_SEARCH),
            "/addons/apptov4/app.php/v1/vod/getVodSearch"
        );
    }
}
