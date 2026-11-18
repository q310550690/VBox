//! 带"规则(rules)"的 HTTP 客户端
//!
//! 所有对源站/解析器的请求都走这里，统一套用：UA、默认请求头、超时。
//! 后续可为单站追加 Referer / Cookie（来自 site.ext）。
//!
//! 编码处理：影视源乱象之一是无 charset 声明却用 GBK 编码。这里统一按
//! Content-Type 声明的 charset 解码；未声明时先按 UTF-8 试，失败再回退 GBK，
//! 避免中文乱码（mojibake）。

use crate::error::{CoreError, Result};
use crate::model::Rules;
use encoding_rs::{Encoding, GBK, UTF_16BE, UTF_16LE, UTF_8};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE, USER_AGENT};
use std::collections::HashMap;
use std::time::Duration;

/// 按 rules 构建一个阻塞客户端（严格校验 TLS 证书）。
pub fn client_for(rules: &Rules) -> Result<reqwest::blocking::Client> {
    build_client(rules, false)
}

/// 构建客户端；`insecure` 为 true 时关闭 TLS 证书校验。
fn build_client(rules: &Rules, insecure: bool) -> Result<reqwest::blocking::Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(&rules.ua).map_err(|e| CoreError::Config(e.to_string()))?,
    );
    for (k, v) in &rules.headers {
        let name =
            HeaderName::from_bytes(k.as_bytes()).map_err(|e| CoreError::Config(e.to_string()))?;
        let val = HeaderValue::from_str(v).map_err(|e| CoreError::Config(e.to_string()))?;
        headers.insert(name, val);
    }

    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(rules.timeout))
        .default_headers(headers);
    if insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    // 通用客户端保持直连：国内点播/网盘站点经海外代理反而可能被阻断或改版。
    // 系统代理只套用在确实需要翻墙的出网源（云盘搜索 TG / PanSou，见 cloudsearch）。
    builder.build().map_err(CoreError::Http)
}

/// 供「上游 TLS 证书已过期、但站点其余部分仍在线」的**个别**家族使用。
///
/// 安全边界：默认客户端（[`client_for`]）始终严格校验证书；本函数只应在已
/// 确认并记录证书异常的家族内调用。当前仅 `csp_Yinyuetai` 使用：其
/// `*.yinyuetai.com` 证书已于 2026-10-30 到期（DigiCert RapidSSL），而 DNS
/// 与 HTTP 服务仍在线；证书续期后应删除该调用。
pub fn client_for_insecure(rules: &Rules) -> Result<reqwest::blocking::Client> {
    build_client(rules, true)
}

/// 从 Content-Type 头里提取声明的 charset（小写），没有则返回 None。
/// 例如 `text/html;charset=gbk` -> Some("gbk")
fn charset_of(headers: &HeaderMap) -> Option<String> {
    let ct = headers.get(CONTENT_TYPE)?.to_str().ok()?;
    for part in ct.split(';').skip(1) {
        // 参数名大小写不敏感，等号两侧允许空白（RFC 2045 BWS），值可能带引号。
        // 旧实现只认精确的 charset= 前缀，会把 Charset = "GBK" 漏成启发式。
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("charset") {
            return Some(
                value
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_ascii_lowercase(),
            );
        }
    }
    None
}

/// 把响应体字节解码成 UTF-8 字符串，兼容 GBK 系编码。
fn decode_body(
    resp: reqwest::blocking::Response,
    url: &str,
    max_bytes: Option<u64>,
) -> Result<String> {
    let charset = charset_of(resp.headers());
    let bytes = match max_bytes {
        None => resp.bytes().map_err(CoreError::Http)?.to_vec(),
        Some(limit) => {
            use std::io::Read;
            let mut buf = Vec::new();
            resp.take(limit + 1)
                .read_to_end(&mut buf)
                .map_err(CoreError::Io)?;
            if buf.len() as u64 > limit {
                return Err(CoreError::Config(format!(
                    "HTTP 响应超过 {limit} 字节上限: {url}"
                )));
            }
            buf
        }
    };

    let encoding: &Encoding = match charset.as_deref() {
        Some("gbk") | Some("gb2312") | Some("gb18030") | Some("x-gbk") => GBK,
        Some("utf-8") | Some("utf8") => UTF_8,
        // 未声明或未知：先试 UTF-8，非法则回退 GBK
        _ => {
            if std::str::from_utf8(&bytes).is_ok() {
                UTF_8
            } else {
                GBK
            }
        }
    };

    // UTF-16 BOM 优先：字节序明确，不参与 UTF-8/GBK 启发式
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Ok(decode_text(UTF_16LE, &bytes));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Ok(decode_text(UTF_16BE, &bytes));
    }

    // 首选：无错解码（不产生替换字符）
    if let Some(s) = encoding.decode_without_bom_handling_and_without_replacement(&bytes) {
        return Ok(strip_bom(&s));
    }
    // 声称为 UTF-8 但实际非法 -> 回退 GBK
    if encoding == UTF_8 {
        if let Some(s) = GBK.decode_without_bom_handling_and_without_replacement(&bytes) {
            return Ok(s.into_owned());
        }
    }
    // 兜底：即便有错也尽量解出（用替换字符填充无法识别的字节）
    Ok(strip_bom(&GBK.decode_without_bom_handling(&bytes).0))
}

/// 去掉解码后残留的 UTF-8 BOM（U+FEFF）。encoding_rs 的 without_bom_handling 不剥离它。
fn strip_bom(s: &str) -> String {
    s.strip_prefix('\u{feff}').unwrap_or(s).to_string()
}

/// 指定编码解码；无法无错解码时退回有损解码，最后剥 BOM。
fn decode_text(enc: &'static Encoding, bytes: &[u8]) -> String {
    match enc.decode_without_bom_handling_and_without_replacement(bytes) {
        Some(s) => strip_bom(&s),
        None => strip_bom(&enc.decode_without_bom_handling(bytes).0),
    }
}

/// 发起 GET 并返回文本；extra 用于追加单站级请求头（如 Referer/Cookie）。
///
/// `retries` 为额外重试次数（来自 `Rules.retry`，默认 2）。第三方影视源普遍不稳定，
/// 启动/切源时偶尔出现连接重置、响应体截断（reqwest 的 `error sending request` /
/// `error decoding response body`），这里做有限次退避重试，避免一次抖动就让整页失败。
pub fn get_text(
    client: &reqwest::blocking::Client,
    url: &str,
    extra: &HashMap<String, String>,
    retries: u32,
) -> Result<String> {
    get_text_limited(client, url, extra, retries, None)
}

/// 与 [`get_text`] 相同，但读取超过 `max_bytes` 的响应体时显式失败，而不是无界读入内存。
pub fn get_text_limited(
    client: &reqwest::blocking::Client,
    url: &str,
    extra: &HashMap<String, String>,
    retries: u32,
    max_bytes: Option<u64>,
) -> Result<String> {
    let attempts = retries.min(5) + 1;
    let mut last_err: Option<CoreError> = None;
    for attempt in 0..attempts {
        match get_text_once(client, url, extra, max_bytes) {
            Ok(text) => return Ok(text),
            Err((e, retryable)) => {
                tracing::warn!(
                    "HTTP 拉取失败（第 {}/{} 次）: {} -> {}",
                    attempt + 1,
                    attempts,
                    url,
                    e
                );
                last_err = Some(e);
                if !retryable {
                    break;
                }
                if attempt + 1 < attempts {
                    std::thread::sleep(Duration::from_millis(300 * (attempt as u64 + 1)));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| CoreError::Config(format!("HTTP 拉取失败: {url}"))))
}

/// 单次 GET 尝试（不做重试）
fn get_text_once(
    client: &reqwest::blocking::Client,
    url: &str,
    extra: &HashMap<String, String>,
    max_bytes: Option<u64>,
) -> std::result::Result<String, (CoreError, bool)> {
    let mut req = client.get(url);
    for (k, v) in extra {
        req = req.header(k, v);
    }
    let started = std::time::Instant::now();
    let event_id = crate::capture::begin("http", "GET", url);
    let resp = match req.send() {
        Ok(r) => r,
        Err(e) => {
            crate::capture::finish(
                event_id,
                None,
                started.elapsed().as_millis(),
                None,
                Some(e.to_string()),
            );
            return Err((CoreError::Http(e), true));
        }
    };
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            None,
            Some(format!("HTTP {status}")),
        );
        let retryable = status == 408 || status == 429 || status >= 500;
        return Err((
            CoreError::Config(format!("HTTP {} 拉取失败: {}", resp.status(), url)),
            retryable,
        ));
    }
    let decoded = decode_body(resp, url, max_bytes).map_err(|e| (e, false));
    match &decoded {
        Ok(text) => crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            Some(text.len()),
            None,
        ),
        Err((e, _)) => crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            None,
            Some(e.to_string()),
        ),
    }
    decoded
}

/// 发起 POST（`application/x-www-form-urlencoded`）并返回文本。
///
/// `extra` 用于追加单站级请求头（Referer/Origin/Cookie 等）；
/// `form` 为表单字段（重复键极少见，用 map 即可）。
pub fn post_form_text(
    client: &reqwest::blocking::Client,
    url: &str,
    extra: &HashMap<String, String>,
    form: &HashMap<String, String>,
) -> Result<String> {
    post_form_text_with_headers(client, url, extra, form).map(|(body, _)| body)
}

/// 与 [`post_form_text`] 相同，但额外返回响应头（用于需要 `Set-Cookie` 串联的站点）。
pub fn post_form_text_with_headers(
    client: &reqwest::blocking::Client,
    url: &str,
    extra: &HashMap<String, String>,
    form: &HashMap<String, String>,
) -> Result<(String, Vec<(String, String)>)> {
    let mut req = client.post(url);
    for (k, v) in extra {
        req = req.header(k, v);
    }
    let started = std::time::Instant::now();
    let event_id = crate::capture::begin("http", "POST", url);
    let resp = match req.form(form).send() {
        Ok(r) => r,
        Err(e) => {
            crate::capture::finish(
                event_id,
                None,
                started.elapsed().as_millis(),
                None,
                Some(e.to_string()),
            );
            return Err(CoreError::Http(e));
        }
    };
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            None,
            Some(format!("HTTP {status}")),
        );
        return Err(CoreError::Config(format!(
            "HTTP {} 拉取失败: {}",
            resp.status(),
            url
        )));
    }
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_ascii_lowercase(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let decoded = decode_body(resp, "", None);
    match &decoded {
        Ok(text) => crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            Some(text.len()),
            None,
        ),
        Err(e) => crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            None,
            Some(e.to_string()),
        ),
    }
    decoded.map(|text| (text, headers))
}

/// 以 JSON 原始 body 发 POST，返回响应文本。
///
/// 与 [`post_form_text_with_headers`] 同构（capture 事件、状态码校验、`decode_body` 解码），
/// 仅把 `form` 编码换成 `Content-Type: application/json` + 原始 body。
pub fn post_json_text(
    client: &reqwest::blocking::Client,
    url: &str,
    extra: &HashMap<String, String>,
    body: &str,
) -> Result<String> {
    let mut req = client.post(url).header("Content-Type", "application/json");
    for (k, v) in extra {
        req = req.header(k, v);
    }
    let started = std::time::Instant::now();
    let event_id = crate::capture::begin("http", "POST", url);
    let resp = match req.body(body.to_string()).send() {
        Ok(r) => r,
        Err(e) => {
            crate::capture::finish(
                event_id,
                None,
                started.elapsed().as_millis(),
                None,
                Some(e.to_string()),
            );
            return Err(CoreError::Http(e));
        }
    };
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            None,
            Some(format!("HTTP {status}")),
        );
        return Err(CoreError::Config(format!(
            "HTTP {} 拉取失败: {}",
            resp.status(),
            url
        )));
    }
    let decoded = decode_body(resp, "", None);
    match &decoded {
        Ok(text) => crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            Some(text.len()),
            None,
        ),
        Err(e) => crate::capture::finish(
            event_id,
            Some(status),
            started.elapsed().as_millis(),
            None,
            Some(e.to_string()),
        ),
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_http::{Header, Response, Server};

    fn serve_once(body: Vec<u8>, ctype: &'static str) -> String {
        let server = Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        std::thread::spawn(move || {
            if let Ok(req) = server.recv() {
                let header = Header::from_bytes("Content-Type", ctype).expect("ctype");
                let _ = req.respond(Response::from_data(body).with_header(header));
            }
        });
        addr
    }

    #[test]
    fn utf8_bom_json_is_stripped_before_parsing() {
        let mut body = vec![0xEF, 0xBB, 0xBF];
        body.extend_from_slice(br#"{"name":"bom"}"#);
        let base = serve_once(body, "application/json; charset=utf-8");
        let client = client_for(&Rules::default()).expect("client");
        let text = get_text(&client, &format!("{base}/x"), &HashMap::new(), 0).expect("get");
        assert!(!text.starts_with('\u{feff}'), "BOM 未被剥离: {:?}", text);
        assert!(
            serde_json::from_str::<serde_json::Value>(&text).is_ok(),
            "BOM 导致 JSON 解析失败: {:?}",
            text
        );
    }

    #[test]
    fn utf16le_bom_is_decoded_as_utf16() {
        let mut body = vec![0xFF, 0xFE];
        for unit in br#"{"name":"utf16"}"#.iter().copied() {
            body.push(unit);
            body.push(0);
        }
        let base = serve_once(body, "application/json");
        let client = client_for(&Rules::default()).expect("client");
        let text = get_text(&client, &format!("{base}/y"), &HashMap::new(), 0).expect("get");
        assert_eq!(text, r#"{"name":"utf16"}"#);
    }
}

#[cfg(test)]
mod tests_retry {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tiny_http::{Response, Server};

    fn serve_status(status: u16, max_requests: usize) -> (String, Arc<AtomicUsize>) {
        let server = Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for _ in 0..max_requests {
                let Ok(_req) = server.recv() else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = _req.respond(Response::empty(status));
            }
        });
        (addr, hits)
    }

    #[test]
    fn http_404_is_not_retried_and_maps_to_network_class() {
        let (base, hits) = serve_status(404, 5);
        let client = client_for(&Rules::default()).expect("client");
        let err = get_text(&client, &format!("{base}/missing"), &HashMap::new(), 3)
            .expect_err("404 must fail");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "404 不应重试");
        let mapped = crate::sources::map_source_error(&crate::model::Site::default(), err);
        assert!(
            mapped.to_string().contains("catvod.net.http"),
            "404 应归因网络层而不是 Parse: {mapped}"
        );
    }

    #[test]
    fn http_500_is_retried_until_attempts_exhausted() {
        let (base, hits) = serve_status(500, 5);
        let client = client_for(&Rules::default()).expect("client");
        let err = get_text(&client, &format!("{base}/boom"), &HashMap::new(), 2)
            .expect_err("500 must fail");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            3,
            "500 应按 retries=2 重试三次"
        );
        assert!(err.to_string().contains("HTTP 500"), "{err}");
    }
}

#[cfg(test)]
mod tests_charset {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_str(value).expect("header"));
        h
    }

    #[test]
    fn charset_parameter_is_case_and_space_insensitive() {
        assert_eq!(
            charset_of(&headers("text/html; Charset = \"GBK\"")),
            Some("gbk".to_string())
        );
        assert_eq!(
            charset_of(&headers("application/json;CHARSET=Utf-8")),
            Some("utf-8".to_string())
        );
        assert_eq!(
            charset_of(&headers("text/html; charset='gb18030'")),
            Some("gb18030".to_string())
        );
    }

    #[test]
    fn absent_or_malformed_charset_returns_none() {
        assert_eq!(charset_of(&headers("text/html")), None);
        assert_eq!(charset_of(&headers("text/html; boundary=xyz")), None);
        assert_eq!(charset_of(&headers("text/html; charset")), None);
    }

    fn serve_once_charset(body: Vec<u8>, ctype: &'static str) -> String {
        use tiny_http::{Header, Response, Server};
        let server = Server::http("127.0.0.1:0").expect("server");
        let addr = format!("http://{}", server.server_addr());
        std::thread::spawn(move || {
            if let Ok(req) = server.recv() {
                let header = Header::from_bytes("Content-Type", ctype).expect("ctype");
                let _ = req.respond(Response::from_data(body).with_header(header));
            }
        });
        addr
    }

    #[test]
    fn utf16be_bom_is_decoded_as_utf16() {
        let mut body = vec![0xFE, 0xFF];
        for unit in br#"{"name":"be"}"#.iter().copied() {
            body.push(0);
            body.push(unit);
        }
        let base = serve_once_charset(body, "application/json");
        let client = client_for(&Rules::default()).expect("client");
        let text = get_text(&client, &format!("{base}/be"), &HashMap::new(), 0).expect("get");
        assert_eq!(text, r#"{"name":"be"}"#);
    }
}
