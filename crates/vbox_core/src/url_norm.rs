//! 订阅 URL 归一化 / 相对路径修正（对齐 jun 版 `ApiConfig.configUrl` 与 `fixContentPath`）

use std::path::{Path, PathBuf};
use url::Url;

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Http,
    File,
    Clan,
    InlineJson,
}

#[derive(Debug, Clone)]
pub struct PreparedInput {
    /// 真正用于请求/读取的地址（已剥离 `;pk;KEY` 与 clan/file 映射）
    pub request_url: String,
    pub pk_key: Option<String>,
    pub kind: InputKind,
    /// 用户原始输入
    pub original: String,
}

/// 解析用户输入的订阅地址：
/// - `X;pk;KEY` → 请求 `X`，密钥 `KEY`（不发送给服务器）
/// - `file://` → 本地文件（返回 `InputKind::File`）
/// - `clan://localhost/...` → `{local_base}/file/...`（需要本地文件服务）
/// - `clan://host[:port]/...` → `http://host[:port]/file/...`
/// - 无 scheme → 补 `http://`
pub fn prepare_input(input: &str, local_base: Option<&str>) -> Result<PreparedInput> {
    let original = input.trim().to_string();
    if original.is_empty() {
        return Err(CoreError::Config("订阅地址为空".into()));
    }
    if original.starts_with('{') {
        return Ok(PreparedInput {
            request_url: original.clone(),
            pk_key: None,
            kind: InputKind::InlineJson,
            original,
        });
    }

    let (addr, pk_key) = match original.split_once(";pk;") {
        Some((left, key)) => (left.trim().to_string(), Some(key.trim().to_string())),
        None => (original.clone(), None),
    };

    if addr.starts_with("clan://") {
        let mapped = clan_to_address(&addr, local_base)?;
        return Ok(PreparedInput {
            request_url: mapped,
            pk_key,
            kind: InputKind::Clan,
            original,
        });
    }

    if let Some(path) = addr.strip_prefix("file://") {
        let path = if cfg!(windows) {
            // file:///E:/x.json → E:/x.json；file://E:/x.json → E:/x.json
            path.trim_start_matches('/')
        } else {
            path
        };
        // Tauri 运行时已启动本地 /file 服务：把本地文件也映射成本地 HTTP URL，
        // 这样配置里的相对图片/jar 才能在 WebView 里加载（file:// 会被 WebView 拦截）。
        if let Some(base) = local_base {
            let normalized = path.replace('\\', "/").replace(' ', "%20");
            return Ok(PreparedInput {
                request_url: format!("{}/file/{}", base.trim_end_matches('/'), normalized),
                pk_key,
                kind: InputKind::Http,
                original,
            });
        }
        return Ok(PreparedInput {
            request_url: path.to_string(),
            pk_key,
            kind: InputKind::File,
            original,
        });
    }

    if addr.starts_with("http://") || addr.starts_with("https://") {
        return Ok(PreparedInput {
            request_url: addr,
            pk_key,
            kind: InputKind::Http,
            original,
        });
    }

    // 本地绝对路径 / 相对路径
    let p = PathBuf::from(&addr);
    if p.is_absolute() || p.exists() {
        if let Some(base) = local_base {
            let normalized = addr.replace('\\', "/").replace(' ', "%20");
            let normalized = normalized.trim_start_matches('/');
            return Ok(PreparedInput {
                request_url: format!("{}/file/{}", base.trim_end_matches('/'), normalized),
                pk_key,
                kind: InputKind::Http,
                original,
            });
        }
        return Ok(PreparedInput {
            request_url: addr,
            pk_key,
            kind: InputKind::File,
            original,
        });
    }

    Ok(PreparedInput {
        request_url: format!("http://{addr}"),
        pk_key,
        kind: InputKind::Http,
        original,
    })
}

/// `clan://` 地址映射（与 jun 一致，但 localhost 必须传本地服务基址）
fn clan_to_address(addr: &str, local_base: Option<&str>) -> Result<String> {
    if let Some(rest) = addr.strip_prefix("clan://localhost/") {
        let base = local_base.ok_or_else(|| {
            CoreError::Config(
                "clan://localhost 需要本地文件服务；桌面端尚未启动本地服务时请改用 file:// 或 http(s)://".into(),
            )
        })?;
        return Ok(format!("{}/file/{}", base.trim_end_matches('/'), rest));
    }
    if let Some(rest) = addr.strip_prefix("clan://") {
        let (host, path) = rest
            .split_once('/')
            .ok_or_else(|| CoreError::Config(format!("无效的 clan:// 地址: {addr}")))?;
        if host.is_empty() {
            return Err(CoreError::Config(format!("无效的 clan:// 地址: {addr}")));
        }
        return Ok(format!("http://{host}/file/{path}"));
    }
    Err(CoreError::Config(format!("无效的 clan:// 地址: {addr}")))
}

/// 把配置正文中的 `"./`、`"../` 相对资源改成绝对 URL（jun 的 `fixContentPath` 行为）
///
/// 注意：为保持与 TVBox 相同的效果，这里同样做**字面量替换**：
/// - `"../` → 配置 URL 的父目录 URL
/// - `"./`  → 配置 URL 所在目录 URL
pub fn fix_relative_paths(content: &str, base: &str) -> String {
    if !content.contains("\"./") && !content.contains("\"../") {
        return content.to_string();
    }
    let (dir, parent) = match base_urls(base) {
        Some(v) => v,
        None => return content.to_string(),
    };
    // 与 jun 一致：先替换 ../，再替换 ./
    content.replace("../", &parent).replace("./", &dir)
}

/// `clanContentFix`：配置本身来自 clan/file 时，把正文里的 clan/file 前缀换成本地服务前缀
pub fn clan_content_fix(content: &str, base: &str) -> String {
    let prefix = base.trim_end_matches('/').to_string();
    content
        .replace("clan://localhost/", &format!("{prefix}/file/"))
        .replace("file://", &format!("{prefix}/file/"))
}

fn base_urls(base: &str) -> Option<(String, String)> {
    if base.starts_with("http://") || base.starts_with("https://") {
        let u = Url::parse(base).ok()?;
        let dir = u.join("./").ok()?;
        let parent = u.join("../").ok()?;
        return Some((dir.to_string(), parent.to_string()));
    }
    let p = Path::new(base);
    let parent_path = p.parent()?;
    let dir = with_trailing_slash(file_url(parent_path));
    let parent = with_trailing_slash(file_url(parent_path.parent().unwrap_or(parent_path)));
    Some((dir, parent))
}

fn with_trailing_slash(mut value: String) -> String {
    if !value.ends_with('/') {
        value.push('/');
    }
    value
}

fn file_url(path: &Path) -> String {
    match Url::from_file_path(path) {
        Ok(u) => u.to_string(),
        Err(_) => path.to_string_lossy().replace('\\', "/"),
    }
}

/// Java `URLEncoder.encode(s, "UTF-8")` 的近似实现：
/// 空格 → `+`，保留 `A-Za-z0-9` 与 `-_.*`，其余按 UTF-8 百分号编码。
pub fn java_url_encode(input: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    const KEEP: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'*');
    let encoded = utf8_percent_encode(input, KEEP).to_string();
    encoded.replace("%20", "+")
}

/// 简单 URL join：用于播放页/解析接口拼接（TVBox 只做字符串拼接，这里保持相同语义）
pub fn join_url(base: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    format!("{}{}", base, url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_http_and_pk() {
        let p = prepare_input("https://a.com/api.json;pk;secret", None).unwrap();
        assert_eq!(p.request_url, "https://a.com/api.json");
        assert_eq!(p.pk_key.as_deref(), Some("secret"));
        assert_eq!(p.kind, InputKind::Http);
    }

    #[test]
    fn prepare_schemeless() {
        let p = prepare_input("a.com/x", None).unwrap();
        assert_eq!(p.request_url, "http://a.com/x");
    }

    #[test]
    fn prepare_clan_remote() {
        let p = prepare_input("clan://192.168.1.2:9978/x/y.json", None).unwrap();
        assert_eq!(p.request_url, "http://192.168.1.2:9978/file/x/y.json");
    }

    #[test]
    fn local_file_maps_to_local_server_when_available() {
        let p = prepare_input("file:///E:/tmp/config.json", Some("http://127.0.0.1:1234")).unwrap();
        assert_eq!(
            p.request_url,
            "http://127.0.0.1:1234/file/E:/tmp/config.json"
        );
        assert_eq!(p.kind, InputKind::Http);
    }

    #[test]
    fn relative_fix_http() {
        let out = fix_relative_paths(
            r#"{"a":"./img/x.png","b":"../y.json"}"#,
            "https://cdn.example.com/cfg/v1/api.json",
        );
        assert!(out.contains("https://cdn.example.com/cfg/v1/img/x.png"));
        assert!(out.contains("https://cdn.example.com/cfg/y.json"));
    }

    #[test]
    fn relative_fix_file_keeps_directory_slash() {
        let out = fix_relative_paths(r#"{"api":"./lib/cat.js"}"#, "file:///E:/cfg/api.json");
        assert!(
            out.contains("file:///E:/cfg/lib/cat.js"),
            "相对路径不应丢掉目录分隔符: {out}"
        );
    }

    #[test]
    fn java_encode() {
        assert_eq!(
            java_url_encode("https://a.com/x?y=1&z=中"),
            "https%3A%2F%2Fa.com%2Fx%3Fy%3D1%26z%3D%E4%B8%AD"
        );
        assert_eq!(java_url_encode("a b"), "a+b");
    }
}
