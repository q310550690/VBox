//! 夸克网盘扫码登录（W13 D3）。
//!
//! 协议来源（`_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/Config.java:1058-1160`、
//! `parser/merge/A/L0.java:339-471`，2026-11-17 真站复核）：
//!
//! 1. `GET https://uop.quark.cn/cas/ajax/getTokenForQrcodeLogin?client_id=532&v=1.2&request_id=`
//!    → `{message:"ok", data:{members:{token}}}`；
//! 2. 二维码内容 = scanLoginPage 地址 + `?token=<token>&client_id=532&ssb=weblogin&...`；
//!    scanLoginPage 地址从 `https://pan.quark.cn` 首页引用的 `*/js/index.js` 里用正则取
//!    `scanLoginPage: "..."`，失败兜底 `https://su.quark.cn/4_eMHBJ`；
//! 3. `GET .../getServiceTicketByQrcodeToken?client_id=532&v=1.2&token=&request_id=`
//!    - `message!="ok"`（如 status=50004001 `Query result is empty`）→ 等待扫码；
//!    - `message=="ok"` → `data.members.service_ticket`；
//! 4. 换 Cookie：`GET https://pan.quark.cn/account/info?st=<service_ticket>&lw=scan`
//!    → 响应 `Set-Cookie`（`__pus`/`__puus` 等）+ body `{"success":true}`。
//!
//! 与 115 的差异：夸克没有独立二维码图片接口，`image_url` 留空，由前端用 `content` 本地渲染。
//! 安全：Cookie 只进入 [`Credential`]，不打日志、不返回前端明文。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use regex::Regex;
use serde_json::Value;

use crate::error::{CoreError, Result};

use super::{
    mask_identity, now_unix, Credential, LoginFlow, LoginOutcome, QrKind, QrTicket, QUARK,
};

/// 夸克 UOP 根。
pub const UOP_BASE: &str = "https://uop.quark.cn";
/// 夸克网盘根。
pub const PAN_BASE: &str = "https://pan.quark.cn";
/// 扫码登录 client_id（网页端）。
pub const CLIENT_ID: &str = "532";
/// 二维码有效期（客户端侧；服务端过期以接口状态为准）。
pub const TTL_SECS: u64 = 300;
/// scanLoginPage 兜底地址。
pub const FALLBACK_SCAN_PAGE: &str = "https://su.quark.cn/4_eMHBJ";

const TOKEN_PATH: &str = "/cas/ajax/getTokenForQrcodeLogin";
const TICKET_PATH: &str = "/cas/ajax/getServiceTicketByQrcodeToken";
const ACCOUNT_INFO_PATH: &str = "/account/info";
const QR_SUFFIX: &str = "&client_id=532&ssb=weblogin&uc_param_str=&uc_biz_str=S%3Acustom%7COPT%3ASAREA%400%7COPT%3AIMMERSIVE%401%7COPT%3ABACK_BTN_STYLE%400";

/// 夸克扫码登录流。
pub struct QuarkLogin {
    client: reqwest::blocking::Client,
    uop_base: String,
    pan_base: String,
}

impl QuarkLogin {
    /// 使用默认真站地址。
    pub fn new(client: reqwest::blocking::Client) -> Self {
        Self::with_base(client, UOP_BASE, PAN_BASE)
    }

    /// 指定 UOP / 网盘根（mock 测试用）。
    pub fn with_base(client: reqwest::blocking::Client, uop_base: &str, pan_base: &str) -> Self {
        Self {
            client,
            uop_base: uop_base.trim_end_matches('/').to_string(),
            pan_base: pan_base.trim_end_matches('/').to_string(),
        }
    }

    fn get_json(&self, url: &str, extra: &HashMap<String, String>) -> Result<Value> {
        let mut req = self.client.get(url);
        for (k, v) in extra {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().map_err(CoreError::Http)?;
        if !resp.status().is_success() {
            return Err(CoreError::Config(format!(
                "夸克登录接口返回 HTTP {}: {url}",
                resp.status()
            )));
        }
        let text = resp.text().map_err(CoreError::Http)?;
        serde_json::from_str(&text)
            .map_err(|e| CoreError::Config(format!("夸克登录响应不是 JSON: {e}")))
    }

    /// 抓 `scanLoginPage` 地址：首页 → index.js → 正则；任何一步失败都回退到兜底地址。
    fn scan_login_page(&self) -> String {
        let home = format!("{}/", self.pan_base);
        let Ok(html) = crate::http::get_text(&self.client, &home, &HashMap::new(), 0) else {
            return FALLBACK_SCAN_PAGE.to_string();
        };
        let Ok(script_re) = Regex::new(r#"src\s*=\s*"([^"]*/js/index\.js)""#) else {
            return FALLBACK_SCAN_PAGE.to_string();
        };
        let Some(caps) = script_re.captures(&html) else {
            return FALLBACK_SCAN_PAGE.to_string();
        };
        let raw = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let script_url = if raw.starts_with("http") {
            raw.to_string()
        } else if let Some(rest) = raw.strip_prefix("//") {
            format!("https://{rest}")
        } else if raw.starts_with('/') {
            format!("{}{raw}", self.pan_base)
        } else {
            format!("{}/{raw}", self.pan_base)
        };
        let Ok(js) = crate::http::get_text(&self.client, &script_url, &HashMap::new(), 0) else {
            return FALLBACK_SCAN_PAGE.to_string();
        };
        let Ok(page_re) = Regex::new(r#"scanLoginPage\s*:\s*"([^"]+)""#) else {
            return FALLBACK_SCAN_PAGE.to_string();
        };
        page_re
            .captures(&js)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| FALLBACK_SCAN_PAGE.to_string())
    }

    /// `service_ticket` 换 Cookie（`/account/info?st=..&lw=scan`）。
    fn exchange_cookie(&self, service_ticket: &str) -> Result<LoginOutcome> {
        let url = format!(
            "{}{ACCOUNT_INFO_PATH}?st={service_ticket}&lw=scan",
            self.pan_base
        );
        let mut req = self
            .client
            .get(&url)
            .header("Referer", "https://pan.quark.cn/");
        // 带上浏览器的 UA（login_client 已设置；此处仅确保 Referer 存在，避免防盗链）。
        req = req.header("Accept", "application/json, text/plain, */*");
        let resp = match req.send() {
            Ok(r) => r,
            Err(e) => return Ok(LoginOutcome::Failure(format!("夸克换取 Cookie 失败: {e}"))),
        };
        let status = resp.status().as_u16();
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
        let text = match resp.text() {
            Ok(t) => t,
            Err(e) => {
                return Ok(LoginOutcome::Failure(format!(
                    "夸克 Cookie 响应读取失败: {e}"
                )))
            }
        };
        if status != 200 {
            return Ok(LoginOutcome::Failure(format!(
                "夸克换取 Cookie 失败: HTTP {status}"
            )));
        }
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !body
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(LoginOutcome::Failure(
                "夸克登录未获取到有效 Cookie".to_string(),
            ));
        }

        let cookie = cookies_from_headers(&headers);
        if cookie.trim().is_empty() {
            return Ok(LoginOutcome::Failure(
                "夸克登录响应缺少 Set-Cookie".to_string(),
            ));
        }

        let data = body
            .get("data")
            .or_else(|| body.get("result"))
            .cloned()
            .unwrap_or(Value::Null);
        let mut cred = Credential::new(QUARK);
        cred.cookie = Some(cookie);
        let identity = data
            .get("member_id")
            .or_else(|| data.get("memberId"))
            .or_else(|| data.get("user_id"))
            .or_else(|| data.get("uid"))
            .and_then(scalar);
        cred.display_name = data
            .get("nickname")
            .or_else(|| data.get("nick"))
            .or_else(|| data.get("user_name"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(id) = &identity {
            cred.account_id = format!("quark:{id}");
            cred.masked_id = Some(mask_identity(id));
            cred.extra.insert("member_id".to_string(), id.clone());
        }
        Ok(LoginOutcome::Success(cred))
    }
}

impl LoginFlow for QuarkLogin {
    fn provider(&self) -> &'static str {
        QUARK
    }

    fn start(&self) -> Result<QrTicket> {
        let url = format!(
            "{}{TOKEN_PATH}?client_id={CLIENT_ID}&v=1.2&request_id={}",
            self.uop_base,
            request_id()
        );
        let value = self.get_json(&url, &HashMap::new())?;
        if value.get("message").and_then(Value::as_str) != Some("ok") {
            let msg = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未知错误");
            return Err(CoreError::Config(format!(
                "夸克二维码 token 获取失败: {msg}"
            )));
        }
        let token = value
            .get("data")
            .and_then(|d| d.get("members"))
            .and_then(|m| m.get("token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Config("夸克二维码响应缺少 token".to_string()))?;
        let scan_page = self.scan_login_page();
        let content = format!("{scan_page}?token={token}{QR_SUFFIX}");

        // 夸克没有独立二维码图片接口：image_url 留空，前端用 content 本地渲染。
        let mut ticket = QrTicket::new("", QrKind::QrCode, content, "", TTL_SECS);
        ticket.state.insert("token".to_string(), token.to_string());
        Ok(ticket)
    }

    fn poll(&self, ticket: &QrTicket) -> Result<LoginOutcome> {
        let token = ticket.state.get("token").cloned().unwrap_or_default();
        if token.is_empty() {
            return Ok(LoginOutcome::Failure("夸克票据缺少 token".to_string()));
        }
        let url = format!(
            "{}{TICKET_PATH}?client_id={CLIENT_ID}&v=1.2&token={token}&request_id={}",
            self.uop_base,
            request_id()
        );
        let value = match self.get_json(&url, &HashMap::new()) {
            Ok(v) => v,
            Err(_) => {
                return Ok(if ticket.is_expired() {
                    LoginOutcome::Expired
                } else {
                    LoginOutcome::Waiting
                })
            }
        };
        // 夸克状态码（官方 JS 错误表，2026-11-17 实测 50004001）：
        // 50004001=等待；2000000=已确认（带 service_ticket）；50004002=过期；
        // 50004003=登录失败；50004004=用户取消。已扫码未确认没有独立码，只能继续等。
        let status = value.get("status").and_then(Value::as_i64).unwrap_or(0);
        match status {
            50004002 => return Ok(LoginOutcome::Expired),
            50004003 => {
                return Ok(LoginOutcome::Failure("夸克登录失败，请重新扫码".to_string()))
            }
            50004004 => {
                return Ok(LoginOutcome::Failure("夸克二维码已被取消".to_string()))
            }
            _ => {}
        }
        if value.get("message").and_then(Value::as_str) != Some("ok") {
            return Ok(if ticket.is_expired() {
                LoginOutcome::Expired
            } else {
                LoginOutcome::Waiting
            });
        }
        let service_ticket = value
            .get("data")
            .and_then(|d| d.get("members"))
            .and_then(|m| m.get("service_ticket"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        match service_ticket {
            Some(st) => self.exchange_cookie(st),
            None => Ok(LoginOutcome::Failure(
                "夸克登录响应缺少 service_ticket".to_string(),
            )),
        }
    }
}

/// 生成 `request_id`（夸克只要求唯一，不校验格式）。
fn request_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seed = format!(
        "{}-{}-{}",
        now_unix(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    format!("{:x}", md5::compute(seed.as_bytes()))
}

/// 从响应头提取 `Set-Cookie` 的 `name=value` 段（去重保序）。
fn cookies_from_headers(headers: &[(String, String)]) -> String {
    let mut out: Vec<String> = Vec::new();
    for (name, value) in headers {
        if name != "set-cookie" {
            continue;
        }
        let Some(pair) = value.split(';').next().map(str::trim) else {
            continue;
        };
        if !pair.contains('=') {
            continue;
        }
        let key = pair.split('=').next().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        // 同名 cookie 后者优先。
        if let Some(idx) = out.iter().position(|p| p.starts_with(&format!("{key}="))) {
            out[idx] = pair.to_string();
        } else {
            out.push(pair.to_string());
        }
    }
    out.join("; ")
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rules;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn test_client() -> reqwest::blocking::Client {
        crate::http::client_for(&Rules::default()).unwrap()
    }

    fn spawn_server<F>(handler: F) -> (String, std::thread::JoinHandle<()>)
    where
        F: Fn(&str) -> (u16, Vec<(String, String)>, String) + Send + 'static,
    {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let (code, headers, body) = handler(req.url());
                let mut resp = tiny_http::Response::from_string(body).with_status_code(code);
                for (k, v) in headers {
                    if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                        resp = resp.with_header(h);
                    }
                }
                let _ = req.respond(resp);
            }
        });
        (base, handle)
    }

    const TOKEN_JSON: &str = r#"{"message":"ok","data":{"members":{"token":"qt-1"}}}"#;
    const WAITING_JSON: &str = r#"{"status":50004001,"message":"Query result is empty","data":{}}"#;
    const TICKET_JSON: &str = r#"{"message":"ok","data":{"members":{"service_ticket":"st-1"}}}"#;

    #[test]
    fn mock_quark_flow_waiting_then_success_with_cookie() {
        let seq = Arc::new(AtomicUsize::new(0));
        let seq2 = seq.clone();
        let (base, _h) = spawn_server(move |url| {
            let path = url.split('?').next().unwrap_or(url);
            if path.ends_with("/cas/ajax/getTokenForQrcodeLogin") {
                (200, vec![], TOKEN_JSON.to_string())
            } else if path.ends_with("/cas/ajax/getServiceTicketByQrcodeToken") {
                if seq2.fetch_add(1, Ordering::SeqCst) == 0 {
                    (200, vec![], WAITING_JSON.to_string())
                } else {
                    (200, vec![], TICKET_JSON.to_string())
                }
            } else if path == "/" {
                (
                    200,
                    vec![("Content-Type".to_string(), "text/html".to_string())],
                    r#"<html><script src="/js/index.js"></script></html>"#.to_string(),
                )
            } else if path == "/js/index.js" {
                (
                    200,
                    vec![(
                        "Content-Type".to_string(),
                        "application/javascript".to_string(),
                    )],
                    r#"var a={scanLoginPage: "https://su.quark.cn/4_eMHBJ"};"#.to_string(),
                )
            } else if path.ends_with("/account/info") {
                (
                    200,
                    vec![
                        (
                            "Set-Cookie".to_string(),
                            "__pus=pus1; Path=/; HttpOnly".to_string(),
                        ),
                        (
                            "Set-Cookie".to_string(),
                            "__puus=puus1; Path=/; HttpOnly".to_string(),
                        ),
                        ("Content-Type".to_string(), "application/json".to_string()),
                    ],
                    r#"{"success":true,"data":{"nickname":"夸克用户","member_id":"m-1"}}"#
                        .to_string(),
                )
            } else {
                (404, vec![], r#"{"message":"not found"}"#.to_string())
            }
        });

        let flow = QuarkLogin::with_base(test_client(), &base, &base);
        let ticket = flow.start().unwrap();
        assert_eq!(ticket.kind, QrKind::QrCode);
        assert!(ticket
            .content
            .starts_with("https://su.quark.cn/4_eMHBJ?token=qt-1"));
        assert!(ticket.image_url.is_empty());
        assert_eq!(ticket.state.get("token").map(String::as_str), Some("qt-1"));

        assert_eq!(flow.poll(&ticket).unwrap(), LoginOutcome::Waiting);
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Success(cred) => {
                assert_eq!(cred.provider, "quark");
                assert_eq!(cred.account_id, "quark:m-1");
                assert_eq!(cred.display_name.as_deref(), Some("夸克用户"));
                let cookie = cred.cookie.unwrap();
                assert!(cookie.contains("__pus=pus1"), "{cookie}");
                assert!(cookie.contains("__puus=puus1"), "{cookie}");
            }
            other => panic!("期望 Success，实际 {other:?}"),
        }
    }

    #[test]
    fn mock_scan_page_fallback_when_js_missing() {
        let (base, _h) = spawn_server(move |url| {
            let path = url.split('?').next().unwrap_or(url);
            if path.ends_with("/cas/ajax/getTokenForQrcodeLogin") {
                (200, vec![], TOKEN_JSON.to_string())
            } else {
                (404, vec![], "nope".to_string())
            }
        });
        let flow = QuarkLogin::with_base(test_client(), &base, &base);
        let ticket = flow.start().unwrap();
        assert!(ticket.content.starts_with(FALLBACK_SCAN_PAGE));
        assert!(ticket.content.contains("token=qt-1"));
    }

    #[test]
    fn mock_status_codes_map_to_terminal_outcomes() {
        let cases = [
            (
                r#"{"status":50004002,"message":"expired","data":{}}"#,
                "expired",
            ),
            (
                r#"{"status":50004003,"message":"login failed","data":{}}"#,
                "failed",
            ),
            (
                r#"{"status":50004004,"message":"canceled","data":{}}"#,
                "cancel",
            ),
        ];
        for (body, kind) in cases {
            let (base, _h) = spawn_server(move |_url| (200, vec![], body.to_string()));
            let flow = QuarkLogin::with_base(test_client(), &base, &base);
            let mut ticket = QrTicket::new("t", QrKind::QrCode, "c", "", TTL_SECS);
            ticket.state.insert("token".to_string(), "qt-1".to_string());
            let outcome = flow.poll(&ticket).unwrap();
            match kind {
                "expired" => assert_eq!(outcome, LoginOutcome::Expired),
                other => match outcome {
                    LoginOutcome::Failure(msg) => assert!(
                        msg.contains(if other == "failed" { "失败" } else { "取消" }),
                        "{msg}"
                    ),
                    other => panic!("期望 Failure，实际 {other:?}"),
                },
            }
        }
    }

    #[test]
    fn start_rejects_bad_token_message() {
        let (base, _h) =
            spawn_server(|_url| (200, vec![], r#"{"message":"error","data":{}}"#.to_string()));
        let flow = QuarkLogin::with_base(test_client(), &base, &base);
        assert!(flow.start().is_err());
    }

    #[test]
    fn poll_without_token_is_failure() {
        let (base, _h) = spawn_server(|_url| (200, vec![], TICKET_JSON.to_string()));
        let flow = QuarkLogin::with_base(test_client(), &base, &base);
        let ticket = QrTicket::new("t", QrKind::QrCode, "c", "", TTL_SECS);
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Failure(msg) => assert!(msg.contains("token"), "{msg}"),
            other => panic!("期望 Failure，实际 {other:?}"),
        }
    }

    #[test]
    fn exchange_without_set_cookie_is_failure() {
        let (base, _h) = spawn_server(move |url| {
            let path = url.split('?').next().unwrap_or(url);
            if path.ends_with("/cas/ajax/getTokenForQrcodeLogin") {
                (200, vec![], TOKEN_JSON.to_string())
            } else if path.ends_with("/cas/ajax/getServiceTicketByQrcodeToken") {
                (200, vec![], TICKET_JSON.to_string())
            } else if path.ends_with("/account/info") {
                (
                    200,
                    vec![("Content-Type".to_string(), "application/json".to_string())],
                    r#"{"success":true,"data":{"nickname":"x"}}"#.to_string(),
                )
            } else {
                (404, vec![], "nope".to_string())
            }
        });
        let flow = QuarkLogin::with_base(test_client(), &base, &base);
        let mut ticket = QrTicket::new("t", QrKind::QrCode, "c", "", TTL_SECS);
        ticket.state.insert("token".to_string(), "qt".into());
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Failure(msg) => assert!(msg.contains("Set-Cookie"), "{msg}"),
            other => panic!("期望 Failure，实际 {other:?}"),
        }
    }

    #[test]
    #[ignore = "live: 需要外网访问 uop.quark.cn / pan.quark.cn（只申请 token，不扫码）"]
    fn live_token_and_waiting_status() {
        let flow = QuarkLogin::new(super::super::login_client().unwrap());
        let ticket = flow.start().unwrap();
        assert!(ticket.content.contains("token="));
        assert!(!ticket.state.get("token").unwrap().is_empty());
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Waiting => {}
            other => panic!("未扫码时应为 Waiting，实际 {other:?}"),
        }
    }
}
