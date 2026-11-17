//! TVBox 配置解密（对齐 jun 版 `ApiConfig.FindResult` + `util/AES.java`）
//!
//! 支持四种形态（按判定顺序）：
//! 1. 明文 JSON 对象
//! 2. `[A-Za-z0-9]{8}**` 标记 + Base64 包装（可再嵌套 AES）
//! 3. `2423…` 的十六进制封装 AES-128-CBC（key/iv 藏在解码后的文本中）
//! 4. URL 后缀 `;pk;KEY` 对应的 AES-128-ECB（hex 密文）
//!
//! 与 Java 实现的差异：
//! - Java 解密失败会静默返回原文；这里返回带上下文的错误，便于 UI 提示和排查。
//! - Java 用平台默认字符集做 `byte[] -> String`；这里固定 UTF-8 lossy，与 Android 常见行为一致。

use aes::Aes128;
use base64::Engine;
#[cfg(test)]
use cbc::cipher::BlockEncryptMut;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit, KeyIvInit};
use serde_json::Value;

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedFormat {
    PlainJson,
    Base64Marker,
    AesCbc2423,
    AesEcbPk,
}

impl DecodedFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            DecodedFormat::PlainJson => "plain",
            DecodedFormat::Base64Marker => "base64-marker",
            DecodedFormat::AesCbc2423 => "aes-cbc-2423",
            DecodedFormat::AesEcbPk => "aes-ecb-pk",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodedConfig {
    pub text: String,
    pub format: DecodedFormat,
    /// 是否做过额外解包（用于日志/UI）
    pub steps: Vec<String>,
}

/// 是否是 JSON 对象（与 `org.json.JSONObject` 一致：只接受 `{...}`，不接受数组）
pub fn is_json_object(text: &str) -> bool {
    matches!(
        serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}').trim()),
        Ok(Value::Object(_))
    )
}

/// 解码配置响应体。
///
/// `pk_key` 来自 URL 的 `;pk;KEY` 后缀；没有则为 `None`。
pub fn decode_config_body(body: &str, pk_key: Option<&str>) -> Result<DecodedConfig> {
    // 不少 TVBox 配置服务器（Apache/静态文件）会带 UTF-8 BOM；浏览器会隐藏它，
    // serde_json 会直接判为非法。这里在判定格式前统一剥掉 BOM。
    let body = body.trim_start_matches('\u{feff}');
    let mut steps = Vec::new();

    // 1) 明文 JSON 对象（容忍首尾空白）
    if is_json_object(body) {
        return Ok(DecodedConfig {
            text: body.to_string(),
            format: DecodedFormat::PlainJson,
            steps,
        });
    }

    // 1.1) 带注释/尾逗号的"脏 JSON"：TVBox 用 Gson（lenient）能直接吃，
    // serde_json 不能。这里只对"看起来是 JSON"的文本做 normalize，避免误伤
    // Base64/hex 密文（其中可能恰好包含 //）。
    let head = body.trim_start();
    if head.starts_with("//")
        || head.starts_with("/*")
        || head.starts_with('{')
        || head.starts_with('[')
    {
        let normalized = crate::normalize::normalize(body);
        if is_json_object(&normalized) {
            steps.push("normalize".into());
            return Ok(DecodedConfig {
                text: normalized,
                format: DecodedFormat::PlainJson,
                steps,
            });
        }
    }

    // 2) `8位字母数字**` + Base64
    let mut content = body.to_string();
    if let Some(pos) = find_marker(body) {
        let b64 = &body[pos + 10..];
        let decoded = decode_base64_lenient(b64).map_err(|e| {
            CoreError::Config(format!(
                "Base64 包装解码失败（标记后内容不是合法 Base64）: {e}"
            ))
        })?;
        content = String::from_utf8_lossy(&decoded).into_owned();
        steps.push("base64-marker".into());
        if is_json_object(&content) {
            return Ok(DecodedConfig {
                text: content,
                format: DecodedFormat::Base64Marker,
                steps,
            });
        }
    }

    let trimmed = content.trim();

    // 3) 2423 十六进制 + AES-CBC
    if trimmed.starts_with("2423") {
        let text = decode_2423(trimmed)?;
        steps.push("aes-cbc-2423".into());
        return Ok(DecodedConfig {
            text,
            format: DecodedFormat::AesCbc2423,
            steps,
        });
    }

    // 4) ;pk; AES-ECB（hex 密文）
    if let Some(key) = pk_key {
        if !key.is_empty() {
            let text = decode_pk_ecb(trimmed, key)?;
            steps.push("aes-ecb-pk".into());
            return Ok(DecodedConfig {
                text,
                format: DecodedFormat::AesEcbPk,
                steps,
            });
        }
    }

    let preview: String = body.chars().take(120).collect();
    Err(CoreError::Config(format!(
        "无法识别的配置格式：既不是 JSON，也不匹配 Base64 标记 / 2423 / ;pk; 加密（响应长度 {}，开头 {:?}）",
        body.len(),
        preview
    )))
}

/// 查找 `[A-Za-z0-9]{8}**` 的起始位置（字节下标）
fn find_marker(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    for i in 0..=bytes.len() - 10 {
        let seg = &bytes[i..i + 8];
        if seg.iter().all(|b| b.is_ascii_alphanumeric())
            && bytes[i + 8] == b'*'
            && bytes[i + 9] == b'*'
        {
            return Some(i);
        }
    }
    None
}

/// Android `Base64.DEFAULT` 的宽松解码：忽略 ASCII 空白，允许缺省 `=` 补位
pub fn decode_base64_lenient(text: &str) -> std::result::Result<Vec<u8>, String> {
    let mut cleaned: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    // 标准字母表；对 URL_SAFE 输入做一次兼容替换（部分配置混用）
    cleaned = cleaned.replace('-', "+").replace('_', "/");
    let pad = (4 - cleaned.len() % 4) % 4;
    cleaned.push_str(&"=".repeat(pad));
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| e.to_string())
}

/// `AES.rightPadding`：trim → 超长截前 N / 等长原样 / 不足右补字符
pub fn right_padding(s: &str, fill: char, n: usize) -> String {
    let s = s.trim();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > n {
        chars[..n].iter().collect()
    } else if chars.len() == n {
        s.to_string()
    } else {
        let mut out: String = s.to_string();
        for _ in 0..(n - chars.len()) {
            out.push(fill);
        }
        out
    }
}

/// 2423 格式：`hex("$#"+key+"#$") + hex(ciphertext) + hex(iv13)`
fn decode_2423(trimmed: &str) -> Result<String> {
    let t: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if !t.is_ascii() {
        return Err(CoreError::Config("2423 配置中包含非 ASCII 字符".into()));
    }
    if t.len() < 26 + 4 {
        return Err(CoreError::Config("2423 配置长度不足".into()));
    }
    let marker = t
        .find("2324")
        .ok_or_else(|| CoreError::Config("2423 配置缺少 2324 分隔符".into()))?;
    let data_start = marker + 4;
    let data_end = t.len() - 26;
    if data_start >= data_end {
        return Err(CoreError::Config("2423 配置密文段为空".into()));
    }
    let data_hex = &t[data_start..data_end];

    let wrapper =
        hex::decode(&t).map_err(|e| CoreError::Config(format!("2423 外层 hex 解码失败: {e}")))?;
    let p = String::from_utf8_lossy(&wrapper).to_string().to_lowercase();

    let key_start = p
        .find("$#")
        .ok_or_else(|| CoreError::Config("2423 配置缺少 $# 密钥起始符".into()))?
        + 2;
    let key_end = p
        .find("#$")
        .ok_or_else(|| CoreError::Config("2423 配置缺少 #$ 密钥结束符".into()))?;
    if key_end < key_start {
        return Err(CoreError::Config("2423 密钥区间不合法".into()));
    }
    let key = right_padding(&p[key_start..key_end], '0', 16);
    let tail: String = p
        .chars()
        .rev()
        .take(13)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.chars().count() != 13 {
        return Err(CoreError::Config("2423 IV 段长度不足 13".into()));
    }
    let iv = right_padding(&tail, '0', 16);

    let cipher_text = hex::decode(data_hex)
        .map_err(|e| CoreError::Config(format!("2423 密文 hex 解码失败: {e}")))?;
    let plain = cbc::Decryptor::<Aes128>::new_from_slices(key.as_bytes(), iv.as_bytes())
        .map_err(|e| CoreError::Config(format!("AES-CBC 参数错误: {e}")))?
        .decrypt_padded_vec_mut::<Pkcs7>(&cipher_text)
        .map_err(|_| CoreError::Config("AES-CBC 解密失败（key/iv/填充不匹配）".into()))?;
    Ok(String::from_utf8_lossy(&plain).into_owned())
}

/// `;pk;` 格式：hex 密文 + AES-ECB
fn decode_pk_ecb(content: &str, cfg_key: &str) -> Result<String> {
    let key = right_padding(cfg_key, '0', 16);
    let bytes = hex::decode(content.trim())
        .map_err(|e| CoreError::Config(format!(";pk; 配置不是合法 hex 密文: {e}")))?;
    let plain = ecb::Decryptor::<Aes128>::new_from_slice(key.as_bytes())
        .map_err(|e| CoreError::Config(format!("AES-ECB 参数错误: {e}")))?
        .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
        .map_err(|_| CoreError::Config("AES-ECB 解密失败（key 或填充不匹配）".into()))?;
    Ok(String::from_utf8_lossy(&plain).into_owned())
}

/// 仅用于测试/fixture 生成：按 2423 格式编码
#[cfg(test)]
pub fn encode_2423(plain: &str, key: &str, iv13: &str) -> String {
    let key16 = right_padding(key, '0', 16);
    let iv16 = right_padding(iv13, '0', 16);
    let cipher = cbc::Encryptor::<Aes128>::new_from_slices(key16.as_bytes(), iv16.as_bytes())
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
    let wrapper = format!("$#{key16}#$");
    let wrapper_hex = hex::encode(wrapper.as_bytes());
    let data_hex = hex::encode(cipher);
    let iv_hex = hex::encode(iv13.as_bytes());
    // 注意：生成端的 key 被 rightPadding 到 16，仍然满足解析端 P 中 $#...#$ 的截取
    format!("{wrapper_hex}{data_hex}{iv_hex}")
}

/// 仅用于测试/fixture 生成：按 ;pk; 格式编码
#[cfg(test)]
pub fn encode_pk_ecb(plain: &str, key: &str) -> String {
    let key16 = right_padding(key, '0', 16);
    let cipher = ecb::Encryptor::<Aes128>::new_from_slice(key16.as_bytes())
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
    hex::encode(cipher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_json_passthrough() {
        let body = r#"{"sites":[]}"#;
        let d = decode_config_body(body, None).unwrap();
        assert_eq!(d.format, DecodedFormat::PlainJson);
        assert_eq!(d.text, body);
    }

    #[test]
    fn plain_json_with_utf8_bom() {
        // 真实案例：http://xhztv.top/dc/ 返回 UTF-8 BOM + JSON（Apache 静态文件）
        let body = "\u{feff}{\"urls\":[{\"name\":\"A\",\"url\":\"http://a\"}]}";
        let d = decode_config_body(body, None).unwrap();
        assert_eq!(d.format, DecodedFormat::PlainJson);
        assert!(is_json_object(&d.text));
    }

    #[test]
    fn dirty_json_with_comments_is_normalized() {
        // 真实案例：不少 TVBox 配置以 // 注释开头，并含尾逗号（Gson 默认可吃）
        let body = r#"// 说明
// 第二行
{
  "sites": [
    {"key":"a","type":1,"api":"http://x",},
  ],
}"#;
        let d = decode_config_body(body, None).unwrap();
        assert_eq!(d.format, DecodedFormat::PlainJson);
        assert!(is_json_object(&d.text));
    }

    #[test]
    fn base64_marker() {
        let body = format!(
            "some image bytes AbCd1234**{}",
            base64::engine::general_purpose::STANDARD.encode(r#"{"sites":[]}"#.as_bytes())
        );
        let d = decode_config_body(&body, None).unwrap();
        assert_eq!(d.format, DecodedFormat::Base64Marker);
        assert!(is_json_object(&d.text));
    }

    #[test]
    fn aes_2423_roundtrip() {
        let plain = r#"{"sites":[{"key":"a","type":1,"api":"http://x"}]}"#;
        let key = "0123456789abcdef";
        let iv = "abcdefghijklm";
        let body = encode_2423(plain, key, iv);
        let d = decode_config_body(&body, None).unwrap();
        assert_eq!(d.format, DecodedFormat::AesCbc2423);
        assert_eq!(d.text, plain);
    }

    #[test]
    fn aes_pk_ecb_roundtrip() {
        let plain = r#"{"sites":[{"key":"a","type":1,"api":"http://x"}]}"#;
        let body = encode_pk_ecb(plain, "mykey");
        let d = decode_config_body(&body, Some("mykey")).unwrap();
        assert_eq!(d.format, DecodedFormat::AesEcbPk);
        assert_eq!(d.text, plain);
    }

    #[test]
    fn unknown_format_error() {
        assert!(decode_config_body("hello world", None).is_err());
        let _ = json!({}); // keep serde_json in scope for feature combinations
    }
}
