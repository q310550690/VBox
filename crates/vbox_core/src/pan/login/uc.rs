//! UC 网盘扫码登录（W14 补齐：与夸克同族 UOP CAS 协议）。
//!
//! 协议来源（`_rea/decompiled/5fc50e17-…/sources/com/github/catvod/spider/merge/m/C0661G.java:645-646`、
//! `FM/O/y.java:243-244`，2026-11-18 真站复核 `api.open.uc.cn` 可达）：
//!
//! 1. `GET https://api.open.uc.cn/cas/ajax/getTokenForQrcodeLogin?pr=ucpro&fr=pc&client_id=386&v=1.2&request_id=`
//!    → `{status:2000000, message:"ok", data:{members:{token}}}`；
//! 2. 二维码内容 = 固定扫码页 `https://su.uc.cn/1_n0ZCv` +
//!    `?uc_param_str=…&token=<token>&client_id=381&uc_biz_str=S%3Acustom%7CC%3Atitlebar_fix`
//!   （jar 中硬编码的 UC 扫码短链，client_id 用 381）；
//! 3. `GET .../getServiceTicketByQrcodeToken?pr=ucpro&fr=pc&client_id=386&v=1.2&token=&request_id=`
//!    - `status=50004001 / message!="ok"` → 等待扫码；
//!    - `message=="ok"` → `data.members.service_ticket`；
//! 4. 换 Cookie：`GET https://drive.uc.cn/account/info?st=<service_ticket>&lw=scan`
//!    → 响应 `Set-Cookie`（`__pus`/`__puus` 等）+ body `{"success":true}`。
//!
//! 与夸克差异：UC 扫码页为 jar 硬编码短链（无需抓首页 JS）；UOP 根为 `api.open.uc.cn`。
//! 手动凭据字段：`drive.uc.cn` 的 `__pus` / `__puus`（整段 Cookie 亦可）。
//! 安全：Cookie 只进入 [`Credential`]，不打日志、不返回前端明文。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::error::{CoreError, Result};

use super::{mask_identity, now_unix, Credential, LoginFlow, LoginOutcome, QrKind, QrTicket, UC};

/// UC UOP 根（CAS 接口）。
pub const UOP_BASE: &str = "https://api.open.uc.cn";
/// UC 网盘根（换 Cookie）。
pub const PAN_BASE: &str = "https://drive.uc.cn";
///  token / 轮询 client_id（网页端）。
pub const CLIENT_ID: &str = "386";
/// 二维码内容里的 client_id（jar 硬编码 381）。
pub const QR_CLIENT_ID: &str = "381";
/// 二维码有效期（客户端侧）。
pub const TTL_SECS: u64 = 300;
/// UC 扫码短链（jar 硬编码）。
pub const SCAN_PAGE: &str = "https://su.uc.cn/1_n0ZCv";

const TOKEN_PATH: &str = "/cas/ajax/getTokenForQrcodeLogin";
const TICKET_PATH: &str = "/cas/ajax/getServiceTicketByQrcodeToken";
const ACCOUNT_INFO_PATH: &str = "/account/info";
const CAS_QUERY: &str = "pr=ucpro&fr=pc&sys=win32";
const QR_SUFFIX: &str = "uc_param_str=dsdnfrpfbivesscpgimibtbmnijblauputogpintnwktprchmt";

/// UC 扫码登录流。
pub struct UcLogin {
    client: reqwest::blocking::Client,
    uop_base: String,
    pan_base: String,
}

impl UcLogin {
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

    fn get_json(&self, url: &str) -> Result<Value> {
        let resp = self.client.get(url).send().map_err(CoreError::Http)?;
        if !resp.status().is_success() {
            return Err(CoreError::Config(format!(
                "UC 登录接口返回 HTTP {}: {url}",
                resp.status()
            )));
        }
        let text = resp.text().map_err(CoreError::Http)?;
        serde_json::from_str(&text).map_err(|e| CoreError::Config(format!("UC 登录响应不是 JSON: {e}")))
    }

    /// `service_ticket` 换 Cookie（`/account/info?st=..&lw=scan`）。
    fn exchange_cookie(&self, service_ticket: &str) -> Result<LoginOutcome> {
        let url = format!(
            "{}{ACCOUNT_INFO_PATH}?st={service_ticket}&lw=scan",
            self.pan_base
        );
        let resp = match self
            .client
            .get(&url)
            .header("Referer", "https://drive.uc.cn/")
            .header("Accept", "application/json, text/plain, */*")
            .send()
        {
            Ok(r) => r,
            Err(e) => return Ok(LoginOutcome::Failure(format!("UC 换取 Cookie 失败: {e}"))),
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
            Err(e) => return Ok(LoginOutcome::Failure(format!("UC Cookie 响应读取失败: {e}"))),
        };
        if status != 200 {
            return Ok(LoginOutcome::Failure(format!(
                "UC 换取 Cookie 失败: HTTP {status}"
            )));
        }
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !body.get("success").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(LoginOutcome::Failure("UC 登录未获取到有效 Cookie".to_string()));
        }
        let cookie = cookies_from_headers(&headers);
        if cookie.trim().is_empty() {
            return Ok(LoginOutcome::Failure("UC 登录响应缺少 Set-Cookie".to_string()));
        }
        let data = body
            .get("data")
            .or_else(|| body.get("result"))
            .cloned()
            .unwrap_or(Value::Null);
        let mut cred = Credential::new(UC);
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
            cred.account_id = format!("uc:{id}");
            cred.masked_id = Some(mask_identity(id));
            cred.extra.insert("member_id".to_string(), id.clone());
        }
        Ok(LoginOutcome::Success(cred))
    }
}

impl LoginFlow for UcLogin {
    fn provider(&self) -> &'static str {
        UC
    }

    fn start(&self) -> Result<QrTicket> {
        let url = format!(
            "{}{TOKEN_PATH}?{CAS_QUERY}&client_id={CLIENT_ID}&v=1.2&request_id={}",
            self.uop_base,
            request_id()
        );
        let value = self.get_json(&url)?;
        if value.get("message").and_then(Value::as_str) != Some("ok") {
            let msg = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未知错误");
            return Err(CoreError::Config(format!("UC 二维码 token 获取失败: {msg}")));
        }
        let token = value
            .get("data")
            .and_then(|d| d.get("members"))
            .and_then(|m| m.get("token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Config("UC 二维码响应缺少 token".to_string()))?;
        let content = format!(
            "{SCAN_PAGE}?{QR_SUFFIX}&token={token}&client_id={QR_CLIENT_ID}&uc_biz_str=S%3Acustom%7CC%3Atitlebar_fix"
        );
        // UC 无独立二维码图片接口：本地把扫码链接渲染成二维码 SVG data URL。
        let image_url = super::qr_data_url(&content);
        let mut ticket = QrTicket::new("", QrKind::QrCode, content, &image_url, TTL_SECS);
        ticket.state.insert("token".to_string(), token.to_string());
        Ok(ticket)
    }

    fn poll(&self, ticket: &QrTicket) -> Result<LoginOutcome> {
        let token = ticket.state.get("token").cloned().unwrap_or_default();
        if token.is_empty() {
            return Ok(LoginOutcome::Failure("UC 票据缺少 token".to_string()));
        }
        let url = format!(
            "{}{TICKET_PATH}?{CAS_QUERY}&client_id={CLIENT_ID}&v=1.2&token={token}&request_id={}",
            self.uop_base,
            request_id()
        );
        let value = match self.get_json(&url) {
            Ok(v) => v,
            Err(_) => {
                return Ok(if ticket.is_expired() {
                    LoginOutcome::Expired
                } else {
                    LoginOutcome::Waiting
                })
            }
        };
        let status = value.get("status").and_then(Value::as_i64).unwrap_or(0);
        match status {
            50004002 => return Ok(LoginOutcome::Expired),
            50004003 => return Ok(LoginOutcome::Failure("UC 登录失败，请重新扫码".to_string())),
            50004004 => return Ok(LoginOutcome::Failure("UC 二维码已被取消".to_string())),
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
            None => Ok(LoginOutcome::Failure("UC 登录响应缺少 service_ticket".to_string())),
        }
    }
}

/// 生成 `request_id`（UC 只要求唯一）。
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

    #[test]
    fn start_builds_uc_scan_content_and_poll_waits_then_succeeds() {
        let (base, _handle) = spawn_server(|url| {
            if url.contains("getTokenForQrcodeLogin") {
                (
                    200,
                    vec![],
                    r#"{"status":2000000,"message":"ok","data":{"members":{"token":"uc-tok"}}}"#
                        .to_string(),
                )
            } else if url.contains("getServiceTicketByQrcodeToken") {
                if url.contains("confirmed=1") {
                    (
                        200,
                        vec![],
                        r#"{"status":2000000,"message":"ok","data":{"members":{"service_ticket":"st-1"}}}"#
                            .to_string(),
                    )
                } else {
                    (
                        200,
                        vec![],
                        r#"{"status":50004001,"message":"Query result is empty"}"#.to_string(),
                    )
                }
            } else if url.contains("/account/info") {
                (
                    200,
                    vec![
                        ("Set-Cookie".to_string(), "__pus=aaa; Path=/".to_string()),
                        ("Set-Cookie".to_string(), "__puus=bbb; Path=/".to_string()),
                    ],
                    r#"{"success":true,"data":{"member_id":"u9","nickname":"UC用户"}}"#.to_string(),
                )
            } else {
                (404, vec![], "{}".to_string())
            }
        });
        let flow = UcLogin::with_base(test_client(), &base, &base);
        let ticket = flow.start().unwrap();
        assert_eq!(ticket.kind, QrKind::QrCode);
        assert!(ticket.content.starts_with(SCAN_PAGE));
        assert!(ticket.content.contains("token=uc-tok"));
        assert!(ticket.content.contains("client_id=381"));
        assert!(ticket.image_url.starts_with("data:image/svg+xml;base64,"));
        // 等待态
        assert!(matches!(flow.poll(&ticket).unwrap(), LoginOutcome::Waiting));
        // 确认态：手动把 token 换成带 confirmed 标记的轮询（mock 用 url 含 confirmed=1）
        let mut confirmed = ticket.clone();
        confirmed
            .state
            .insert("token".to_string(), "uc-tok&confirmed=1".to_string());
        match flow.poll(&confirmed).unwrap() {
            LoginOutcome::Success(cred) => {
                assert_eq!(cred.provider, UC);
                let cookie = cred.cookie.unwrap_or_default();
                assert!(cookie.contains("__pus=aaa"));
                assert!(cookie.contains("__puus=bbb"));
                assert_eq!(cred.display_name.as_deref(), Some("UC用户"));
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[test]
    fn poll_maps_cancel_and_expire_codes() {
        let (base, _handle) = spawn_server(|url| {
            if url.contains("cancel=1") {
                (200, vec![], r#"{"status":50004004,"message":"cancel"}"#.to_string())
            } else {
                (200, vec![], r#"{"status":50004002,"message":"expire"}"#.to_string())
            }
        });
        let flow = UcLogin::with_base(test_client(), &base, &base);
        let mut t = QrTicket::new("", QrKind::QrCode, "x", "", TTL_SECS);
        t.state.insert("token".to_string(), "cancel=1".to_string());
        assert!(matches!(flow.poll(&t).unwrap(), LoginOutcome::Failure(_)));
        t.state.insert("token".to_string(), "expire=1".to_string());
        assert!(matches!(flow.poll(&t).unwrap(), LoginOutcome::Expired));
    }
}
