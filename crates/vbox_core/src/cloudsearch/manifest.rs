//! 远端频道清单（W14 §11 P3）：签名校验、解析与归一化。
//!
//! 目的：内置 `sources.json` 里的 Telegram 频道会失效，桌面端需要能「热更新」频道清单，
//! 但清单来自网络，必须防篡改。这里采用 **RSA PKCS#1 v1.5 + SHA-256** 签名：
//!
//! - 清单是一个 JSON 对象，顶层多一个 `signature` 字段（base64 编码的签名）；
//! - 签名载荷 = **移除 `signature` 键后的紧凑 JSON**（`serde_json` 的 `Map` 默认按 key
//!   排序，因此签名方与校验方用同样的规则即可得到完全一致的字节）；
//! - 校验方只需要一份 RSA 公钥（PEM），公钥由客户端配置；
//! - 默认关闭：未配置 URL + 公钥时不走远端清单，完全沿用内置清单。
//!
//! 清单格式：
//! ```json
//! {
//!   "version": "2026.11.20",
//!   "updated_at": 1795000000,
//!   "channels": [
//!     {"handle": "yunpan_channel", "enabled": true, "weight": 10}
//!   ],
//!   "signature": "base64…"
//! }
//! ```
//!
//! 签名用命令行示例（Go/Python 均可，只要遵循上面的规范化规则）：
//!
//! ```text
//! payload = json.dumps(obj_without_signature, separators=(',', ':'), sort_keys=True)
//! signature = base64(rsa_pkcs1v15_sha256_sign(payload))
//! ```
//!
//! 本模块只做纯解析/验签，不做网络请求。

use std::collections::BTreeSet;

use base64::Engine as _;
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::Verifier;
use rsa::RsaPublicKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;

use crate::error::CoreError;

/// 清单原文上限（256 KiB）。超过直接拒绝，避免被超大响应拖垮。
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// 单份清单最多允许的频道数。
pub const MAX_CHANNELS: usize = 200;

/// 顶层签名字段名。
pub const SIGNATURE_FIELD: &str = "signature";

/// 清单里的单个频道。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSpec {
    /// Telegram 公开频道 handle（可带 `@` 或 `t.me/` 前缀，解析时会归一化）。
    pub handle: String,
    /// 是否启用（缺省为 `true`）。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 可选排序权重（越大越靠前；仅作为提示，当前内核按清单顺序抓取）。
    #[serde(default)]
    pub weight: Option<u32>,
}

/// 远端频道清单（已通过签名校验的强类型形式）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcesManifest {
    /// 清单版本（展示用；签名校验不会依赖它）。
    pub version: String,
    /// 签名时间（Unix 秒；可选，仅用于展示）。
    #[serde(default)]
    pub updated_at: Option<i64>,
    /// 频道列表。
    pub channels: Vec<ChannelSpec>,
}

/// 验签并解析远端频道清单。
///
/// 失败时返回 [`CoreError::Config`]（可读、不会回显清单内容或 URL）或
/// [`CoreError::Json`]（结构不合法）。
pub fn verify_sources_manifest(
    raw: &str,
    public_key_pem: &str,
) -> Result<SourcesManifest, CoreError> {
    if raw.len() > MAX_MANIFEST_BYTES {
        return Err(CoreError::Config(format!(
            "远端频道清单过大（上限 {MAX_MANIFEST_BYTES} 字节）"
        )));
    }

    let value: Value = serde_json::from_str(raw).map_err(|err| CoreError::Json {
        path: "manifest".to_string(),
        msg: err.to_string(),
    })?;

    let signature_b64 = value
        .get(SIGNATURE_FIELD)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Config("远端频道清单缺少 signature 字段".to_string()))?;
    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim())
        .map_err(|_| CoreError::Config("远端频道清单签名不是合法的 base64".to_string()))?;
    let signature = Signature::try_from(signature_bytes.as_slice())
        .map_err(|_| CoreError::Config("远端频道清单签名长度无效".to_string()))?;

    let canonical = canonical_payload_from_value(&value)?;
    let public_key = RsaPublicKey::from_public_key_pem(public_key_pem.trim()).map_err(|_| {
        CoreError::Config("频道清单公钥无效（需要 PEM 编码的 RSA 公钥）".to_string())
    })?;
    let verifying_key = VerifyingKey::<Sha256>::new(public_key);
    verifying_key
        .verify(canonical.as_bytes(), &signature)
        .map_err(|_| CoreError::Config("远端频道清单签名校验失败".to_string()))?;

    // 签名通过后再做形状校验：结构错误单独报错，避免把「签名不对」误报成「格式不对」。
    let manifest: SourcesManifest =
        serde_json::from_value(value).map_err(|err| CoreError::Json {
            path: "manifest".to_string(),
            msg: err.to_string(),
        })?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

/// 从原始清单 JSON 计算「签名载荷」字符串（移除 `signature` 后的紧凑 JSON）。
///
/// 公开出来方便签名工具/测试复用，保证两端字节完全一致。
pub fn canonical_payload(raw: &str) -> Result<String, CoreError> {
    let value: Value = serde_json::from_str(raw).map_err(|err| CoreError::Json {
        path: "manifest".to_string(),
        msg: err.to_string(),
    })?;
    canonical_payload_from_value(&value)
}

/// 取清单中「已启用」的频道 handle（保留清单顺序）。
pub fn channels_from_manifest(manifest: &SourcesManifest) -> Vec<String> {
    manifest
        .channels
        .iter()
        .filter(|channel| channel.enabled)
        .map(|channel| channel.handle.clone())
        .collect()
}

fn canonical_payload_from_value(value: &Value) -> Result<String, CoreError> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| CoreError::Config("远端频道清单必须是 JSON 对象".to_string()))?;
    object.remove(SIGNATURE_FIELD);
    serde_json::to_string(&Value::Object(object)).map_err(|err| CoreError::Json {
        path: "manifest".to_string(),
        msg: err.to_string(),
    })
}

fn default_enabled() -> bool {
    true
}

fn validate_manifest(manifest: &SourcesManifest) -> Result<(), CoreError> {
    if manifest.version.trim().is_empty() || manifest.version.len() > 64 {
        return Err(CoreError::Config(
            "远端频道清单 version 不合法（1–64 字符）".to_string(),
        ));
    }
    if !manifest
        .version
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(CoreError::Config(
            "远端频道清单 version 只能包含字母、数字、点、下划线与短横线".to_string(),
        ));
    }
    if manifest.channels.is_empty() {
        return Err(CoreError::Config("远端频道清单没有任何频道".to_string()));
    }
    if manifest.channels.len() > MAX_CHANNELS {
        return Err(CoreError::Config(format!(
            "远端频道清单频道数超限（上限 {MAX_CHANNELS}）"
        )));
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for channel in &manifest.channels {
        let normalized = normalize_handle(&channel.handle).ok_or_else(|| {
            CoreError::Config(format!("远端频道清单包含非法 handle: {}", channel.handle))
        })?;
        if !seen.insert(normalized) {
            return Err(CoreError::Config(format!(
                "远端频道清单包含重复频道: {}",
                channel.handle
            )));
        }
        if let Some(weight) = channel.weight {
            if weight > 1_000 {
                return Err(CoreError::Config(format!(
                    "远端频道清单频道权重超限（上限 1000）: {}",
                    channel.handle
                )));
            }
        }
    }
    Ok(())
}

/// 归一化 Telegram 频道 handle：去空白、去 `@` / `https://t.me/` / `t.me/` 前缀、转小写。
///
/// 返回 `None` 表示不是合法 handle（仅允许 `[a-z0-9_]`，长度 4–64）。
pub fn normalize_handle(raw: &str) -> Option<String> {
    let mut handle = raw.trim();
    for prefix in ["https://t.me/", "http://t.me/", "t.me/"] {
        if let Some(rest) = handle.strip_prefix(prefix) {
            handle = rest;
            break;
        }
    }
    let handle = handle.trim().trim_start_matches('@').trim();
    if handle.len() < 4 || handle.len() > 64 {
        return None;
    }
    if !handle
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(handle.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::{EncodePublicKey, LineEnding};
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::RsaPrivateKey;
    use std::sync::OnceLock;

    struct TestKeys {
        private: RsaPrivateKey,
        pem: String,
    }

    fn test_keys() -> &'static TestKeys {
        static KEYS: OnceLock<TestKeys> = OnceLock::new();
        KEYS.get_or_init(|| {
            let mut rng = rand::thread_rng();
            let private = RsaPrivateKey::new(&mut rng, 2048).expect("生成测试 RSA 密钥");
            let pem = private
                .to_public_key()
                .to_public_key_pem(LineEnding::LF)
                .expect("导出测试公钥 PEM");
            TestKeys { private, pem }
        })
    }

    fn sample_manifest() -> SourcesManifest {
        SourcesManifest {
            version: "2026.11.17".to_string(),
            updated_at: Some(1_794_000_000),
            channels: vec![
                ChannelSpec {
                    handle: "@yunpan_channel".to_string(),
                    enabled: true,
                    weight: Some(10),
                },
                ChannelSpec {
                    handle: "t.me/netdisk_daily".to_string(),
                    enabled: true,
                    weight: None,
                },
                ChannelSpec {
                    handle: "paused_channel".to_string(),
                    enabled: false,
                    weight: None,
                },
            ],
        }
    }

    fn manifest_without_signature(manifest: &SourcesManifest) -> Value {
        serde_json::to_value(manifest).expect("序列化清单")
    }

    fn sign(manifest: &SourcesManifest, private: &RsaPrivateKey) -> String {
        let mut value = manifest_without_signature(manifest);
        let canonical = canonical_payload_from_value(&value).expect("计算签名载荷");
        let signing_key = SigningKey::<Sha256>::new(private.clone());
        let signature = signing_key.sign(canonical.as_bytes());
        let signature_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
        value
            .as_object_mut()
            .expect("清单对象")
            .insert(SIGNATURE_FIELD.to_string(), Value::String(signature_b64));
        serde_json::to_string(&value).expect("序列化签名清单")
    }

    #[test]
    fn verifies_valid_signed_manifest() {
        let keys = test_keys();
        let manifest = sample_manifest();
        let raw = sign(&manifest, &keys.private);
        let verified = verify_sources_manifest(&raw, &keys.pem).expect("验签通过");
        assert_eq!(verified, manifest);
        let channels = channels_from_manifest(&verified);
        assert_eq!(channels, vec!["@yunpan_channel", "t.me/netdisk_daily"]);
    }

    #[test]
    fn canonical_payload_is_stable_and_excludes_signature() {
        let keys = test_keys();
        let manifest = sample_manifest();
        let raw = sign(&manifest, &keys.private);
        let canonical = canonical_payload(&raw).expect("计算载荷");
        assert!(!canonical.contains(SIGNATURE_FIELD));
        // 两个 key 顺序不同的等价 JSON 应得到同一份载荷（serde_json Map 按 key 排序）。
        let reordered = format!(
            r#"{{"channels":{},"updated_at":{},"version":{},"signature":"x"}}"#,
            serde_json::to_string(&serde_json::to_value(&manifest.channels).unwrap()).unwrap(),
            manifest.updated_at.unwrap(),
            serde_json::to_string(&manifest.version).unwrap()
        );
        assert_eq!(canonical, canonical_payload(&reordered).expect("计算载荷"));
    }

    #[test]
    fn rejects_tampered_payload() {
        let keys = test_keys();
        let manifest = sample_manifest();
        let raw = sign(&manifest, &keys.private);
        let tampered = raw.replace("2026.11.17", "2026.11.18");
        let err = verify_sources_manifest(&tampered, &keys.pem).expect_err("篡改应失败");
        assert!(matches!(err, CoreError::Config(message) if message.contains("签名校验失败")));
    }

    #[test]
    fn rejects_wrong_public_key() {
        let keys = test_keys();
        let manifest = sample_manifest();
        let raw = sign(&manifest, &keys.private);
        let mut rng = rand::thread_rng();
        let other = RsaPrivateKey::new(&mut rng, 2048).expect("生成第二把测试密钥");
        let other_pem = other
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("导出第二把公钥");
        let err = verify_sources_manifest(&raw, &other_pem).expect_err("错误公钥应失败");
        assert!(matches!(err, CoreError::Config(message) if message.contains("签名校验失败")));
    }

    #[test]
    fn rejects_oversized_manifest() {
        let raw = " ".repeat(MAX_MANIFEST_BYTES + 1);
        let err = verify_sources_manifest(&raw, "not-a-key").expect_err("超大清单应失败");
        assert!(matches!(err, CoreError::Config(message) if message.contains("过大")));
    }

    #[test]
    fn rejects_invalid_shape_after_signature() {
        let keys = test_keys();
        let empty = SourcesManifest {
            version: "v1".to_string(),
            updated_at: None,
            channels: Vec::new(),
        };
        let err = verify_sources_manifest(&sign(&empty, &keys.private), &keys.pem)
            .expect_err("空频道应失败");
        assert!(matches!(err, CoreError::Config(message) if message.contains("没有任何频道")));

        let bad_handle = SourcesManifest {
            version: "v1".to_string(),
            updated_at: None,
            channels: vec![ChannelSpec {
                handle: "bad handle!".to_string(),
                enabled: true,
                weight: None,
            }],
        };
        let err = verify_sources_manifest(&sign(&bad_handle, &keys.private), &keys.pem)
            .expect_err("非法 handle 应失败");
        assert!(matches!(err, CoreError::Config(message) if message.contains("非法 handle")));
    }

    #[test]
    fn rejects_missing_signature_and_bad_pem() {
        let keys = test_keys();
        let raw = serde_json::to_string(&sample_manifest()).expect("序列化");
        let err = verify_sources_manifest(&raw, &keys.pem).expect_err("缺少签名应失败");
        assert!(matches!(err, CoreError::Config(message) if message.contains("signature")));

        let bad_pem = "-----BEGIN PUBLIC KEY-----\nnope\n-----END PUBLIC KEY-----\n";
        let signed = sign(&sample_manifest(), &keys.private);
        let err = verify_sources_manifest(&signed, bad_pem).expect_err("坏公钥应失败");
        assert!(matches!(err, CoreError::Config(message) if message.contains("公钥无效")));
    }

    #[test]
    fn normalize_handle_variants() {
        assert_eq!(
            normalize_handle(" @YunPan_Channel "),
            Some("yunpan_channel".to_string())
        );
        assert_eq!(
            normalize_handle("https://t.me/netdisk_daily"),
            Some("netdisk_daily".to_string())
        );
        assert_eq!(normalize_handle("abc"), None);
        assert_eq!(normalize_handle("bad handle"), None);
    }
}
