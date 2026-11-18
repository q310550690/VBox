//! 系统代理探测。
//!
//! 桌面端用户的网络环境经常需要代理才能访问 Telegram / GitHub 等站点；VBox 的
//! 出网请求（订阅、云盘搜索、解析）统一走 [`crate::http`] 或云盘源的独立
//! client，这里把「系统代理」探测集中到一处：
//!
//! 1. 环境变量：`HTTPS_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `HTTP_PROXY`（大小写都认）；
//! 2. Windows：读取注册表 `HKCU\...\Internet Settings` 的 `ProxyEnable` / `ProxyServer`
//!    （即「设置 → 网络和 Internet → 代理」里配置的系统代理）。
//!
//! 只支持 HTTP(S) 代理（reqwest 未启用 socks feature）；检测结果进程内缓存一次。

use std::sync::OnceLock;

static SYSTEM_PROXY: OnceLock<Option<String>> = OnceLock::new();

/// 返回形如 `http://127.0.0.1:7897` 的代理地址；没有系统代理时返回 `None`。
pub fn system_proxy_url() -> Option<String> {
    SYSTEM_PROXY.get_or_init(detect_proxy).clone()
}

/// 给 reqwest 客户端构建器套用系统代理（未检测到代理时原样返回）。
///
/// 本机回环地址不经过代理，避免把 local server / WebDAV 之类的本地请求发到代理。
pub fn apply_system_proxy(
    builder: reqwest::blocking::ClientBuilder,
) -> reqwest::blocking::ClientBuilder {
    let Some(url) = system_proxy_url() else {
        return builder;
    };
    match reqwest::Proxy::all(&url) {
        Ok(proxy) => {
            builder.proxy(proxy.no_proxy(reqwest::NoProxy::from_string("localhost,127.0.0.1,::1")))
        }
        Err(err) => {
            tracing::warn!(%url, %err, "系统代理地址无效，已忽略");
            builder
        }
    }
}

fn detect_proxy() -> Option<String> {
    env_proxy().or_else(windows_registry_proxy)
}

/// 从环境变量读取代理（`HTTPS_PROXY` 优先于 `HTTP_PROXY`）。
pub fn env_proxy() -> Option<String> {
    const KEYS: [&str; 6] = [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ];
    for key in KEYS {
        let Ok(raw) = std::env::var(key) else {
            continue;
        };
        if let Some(url) = normalize_proxy_url(&raw) {
            return Some(url);
        }
    }
    None
}

/// 解析 Windows 系统代理设置里的 `ProxyServer` 值。
///
/// 支持三种常见写法：
/// - `127.0.0.1:7897`
/// - `http=127.0.0.1:7897;https=127.0.0.1:7897`
/// - `http://127.0.0.1:7897`
///
/// 优先使用 `https=` 条目（VBox 的出网请求绝大多数是 HTTPS）；只有 socks 条目
/// 时返回 `None`（reqwest 未启用 socks feature）。
pub fn parse_proxy_server(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mut http: Option<String> = None;
    let mut https: Option<String> = None;
    let mut bare: Option<String> = None;
    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('=') {
            Some((scheme, addr)) => match scheme.trim().to_ascii_lowercase().as_str() {
                "https" => https = normalize_proxy_url(addr),
                "http" => http = normalize_proxy_url(addr),
                // socks / ftp 等：reqwest 不能直接用，跳过
                _ => {}
            },
            None if bare.is_none() => bare = normalize_proxy_url(part),
            None => {}
        }
    }
    https.or(http).or(bare)
}

/// 归一化代理地址：补 `http://` 前缀，只接受 http/https，去掉末尾 `/`。
pub fn normalize_proxy_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let url = reqwest::Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str().unwrap_or_default().is_empty() {
        return None;
    }
    Some(candidate.trim_end_matches('/').to_string())
}

#[cfg(windows)]
fn windows_registry_proxy() -> Option<String> {
    let enabled = reg_value("ProxyEnable")?;
    let enabled = enabled.trim();
    // REG_DWORD 输出 `0x1`；部分系统输出 `1`
    if enabled != "0x1" && enabled != "1" {
        return None;
    }
    let server = reg_value("ProxyServer")?;
    parse_proxy_server(&server)
}

#[cfg(not(windows))]
fn windows_registry_proxy() -> Option<String> {
    None
}

/// 读取 `HKCU\...\Internet Settings` 下某个值（通过 `reg query`，只在首次探测时执行）。
#[cfg(windows)]
fn reg_value(name: &str) -> Option<String> {
    use std::process::Command;
    let output = Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "/v",
            name,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        let rest = rest.trim_start();
        let mut parts = rest.splitn(2, char::is_whitespace);
        let _kind = parts.next()?;
        let value = parts.next()?.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_proxy_url_accepts_host_port_and_http() {
        assert_eq!(
            normalize_proxy_url("127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        assert_eq!(
            normalize_proxy_url("http://127.0.0.1:7897/").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        assert_eq!(
            normalize_proxy_url("https://proxy.example:8443").as_deref(),
            Some("https://proxy.example:8443")
        );
    }

    #[test]
    fn normalize_proxy_url_rejects_socks_and_empty() {
        assert_eq!(normalize_proxy_url(""), None);
        assert_eq!(normalize_proxy_url("   "), None);
        assert_eq!(normalize_proxy_url("socks5://127.0.0.1:1080"), None);
        assert_eq!(normalize_proxy_url("socks=127.0.0.1:1080"), None);
    }

    #[test]
    fn parse_proxy_server_prefers_https_entry() {
        assert_eq!(
            parse_proxy_server("http=127.0.0.1:8080;https=127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        assert_eq!(
            parse_proxy_server("127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        assert_eq!(
            parse_proxy_server("http://127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
    }

    #[test]
    fn parse_proxy_server_skips_socks_only() {
        assert_eq!(parse_proxy_server("socks=127.0.0.1:1080"), None);
        assert_eq!(
            parse_proxy_server("socks5=127.0.0.1:1080;ftp=1.2.3.4:21"),
            None
        );
    }
}
