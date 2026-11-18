//! 迅雷网盘设备码登录（W13 D3，OAuth 2.0 Device Authorization Grant）。
//!
//! 协议来源（2026-11-17 真站实测）：
//!
//! 1. `POST {auth_base}auth/device/code`，JSON `{"client_id":"Xp6vsxz_7IYVw2BB"}`
//!    → `{device_code, user_code, expires_in:120, interval:2, verification_url,
//!    verification_uri_complete, short_uri_complete}`；
//! 2. 用户打开 `verification_uri_complete` 输入/确认 `user_code`；
//! 3. `POST {auth_base}auth/token`，JSON
//!    `{"grant_type":"urn:ietf:params:oauth:grant-type:device_code",
//!      "device_code":.., "client_id":..}`：
//!    - 400 `authorization_pending` → 等待；
//!    - 400 `slow_down`/HTTP 428 → 等待（客户端降速）；
//!    - 400 `expired_token` → 过期；
//!    - 400 `access_denied` → 用户拒绝；
//!    - 200 → `access_token` + `refresh_token` + `expires_in`（成功）。
//!
//! 注意：设备码换 token **不能**带 `client_secret`。真站实测（2026-11-17）带 secret
//! 会被网关以 `HTTP 403 permission_denied [Danger], Please Do Not Save Your
//! client_secret in browser...` 拒绝；刷新 token（`pan/xunlei.rs`）仍按既有实现带 secret。
//!
//! 安全：access/refresh token 只进入 [`Credential`]，不打日志、不返回前端明文。

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::pan::xunlei::{AUTH_BASE, XUNLEI_CLIENT_ID};

use super::{
    mask_identity, now_unix, qr_data_url, Credential, LoginFlow, LoginOutcome, QrKind, QrTicket,
    XUNLEI,
};

const DEVICE_CODE_PATH: &str = "/auth/device/code";
const TOKEN_PATH: &str = "/auth/token";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// verification_uri_complete 缺失时的兜底验证页。
const FALLBACK_VERIFY: &str = "https://xluser-ssl.xunlei.com/__/auth/device/";

/// 迅雷设备码登录流。
pub struct XunleiLogin {
    client: reqwest::blocking::Client,
    base: String,
    client_id: String,
}

impl XunleiLogin {
    /// 使用默认真站地址与官方 client_id。
    pub fn new(client: reqwest::blocking::Client) -> Self {
        Self::with_base(client, AUTH_BASE)
    }

    /// 指定 API 根（mock 测试用）。
    pub fn with_base(client: reqwest::blocking::Client, base: &str) -> Self {
        Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            client_id: XUNLEI_CLIENT_ID.to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// 发 JSON POST 并返回（状态码, 响应文本）；非 2xx 也要读 body 以识别 OAuth 错误码。
    fn post_json(&self, path: &str, body: &Value) -> Result<(u16, String)> {
        let resp = self
            .client
            .post(self.url(path))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(CoreError::Http)?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().map_err(CoreError::Http)?;
        Ok((status, String::from_utf8_lossy(&bytes).to_string()))
    }
}

impl LoginFlow for XunleiLogin {
    fn provider(&self) -> &'static str {
        XUNLEI
    }

    fn start(&self) -> Result<QrTicket> {
        let body = json!({ "client_id": self.client_id });
        let (status, text) = self.post_json(DEVICE_CODE_PATH, &body)?;
        if status != 200 {
            return Err(CoreError::Config(format!(
                "迅雷设备码请求失败: HTTP {status}"
            )));
        }
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| CoreError::Config(format!("迅雷设备码响应不是 JSON: {e}")))?;
        let device_code = value
            .get("device_code")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Config("迅雷设备码响应缺少 device_code".to_string()))?;
        let user_code = value
            .get("user_code")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Config("迅雷设备码响应缺少 user_code".to_string()))?;
        let expires_in = value
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(120)
            .clamp(30, 900);
        let interval = value
            .get("interval")
            .and_then(Value::as_u64)
            .unwrap_or(2)
            .max(1);
        let verify_url = value
            .get("verification_uri_complete")
            .and_then(Value::as_str)
            .or_else(|| value.get("verification_url").and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .unwrap_or(FALLBACK_VERIFY);

        // DeviceCode 语义：content = 用户要输入/确认的 user_code，
        // image_url = 验证页地址（含 user_code 的 complete 链接），
        // qr_image = 该验证链接本地渲染的二维码（手机迅雷扫码后在手机上确认授权）。
        let scan_target = value
            .get("short_uri_complete")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(verify_url);
        let mut ticket = QrTicket::new("", QrKind::DeviceCode, user_code, verify_url, expires_in);
        ticket.qr_image = qr_data_url(scan_target);
        ticket
            .state
            .insert("device_code".to_string(), device_code.to_string());
        ticket
            .state
            .insert("interval".to_string(), interval.to_string());
        ticket
            .state
            .insert("client_id".to_string(), self.client_id.clone());
        ticket
            .state
            .insert("verification_url".to_string(), verify_url.to_string());
        Ok(ticket)
    }

    fn poll(&self, ticket: &QrTicket) -> Result<LoginOutcome> {
        let device_code = ticket.state.get("device_code").cloned().unwrap_or_default();
        if device_code.is_empty() {
            return Ok(LoginOutcome::Failure(
                "迅雷票据缺少 device_code".to_string(),
            ));
        }
        let body = json!({
            "grant_type": DEVICE_GRANT,
            "device_code": device_code,
            "client_id": self.client_id,
        });
        // 网络抖动视为「暂无变化」，由票据有效期兜底。
        let (status, text) = match self.post_json(TOKEN_PATH, &body) {
            Ok(v) => v,
            Err(_) => {
                return Ok(if ticket.is_expired() {
                    LoginOutcome::Expired
                } else {
                    LoginOutcome::Waiting
                })
            }
        };
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

        if status == 200 {
            return Ok(self.success_from_value(&value));
        }

        let error = value.get("error").and_then(Value::as_str).unwrap_or("");
        match error {
            "authorization_pending" | "slow_down" => Ok(if ticket.is_expired() {
                LoginOutcome::Expired
            } else {
                LoginOutcome::Waiting
            }),
            "expired_token" => Ok(LoginOutcome::Expired),
            "access_denied" => Ok(LoginOutcome::Failure("迅雷授权已被拒绝".to_string())),
            _ if status == 428 => Ok(LoginOutcome::Waiting),
            _ => {
                let desc = value
                    .get("error_description")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("未知错误");
                Ok(LoginOutcome::Failure(format!(
                    "迅雷登录失败: HTTP {status} {error} {desc}"
                )))
            }
        }
    }
}

impl XunleiLogin {
    fn success_from_value(&self, value: &Value) -> LoginOutcome {
        let data = value.get("data").filter(|d| d.is_object()).unwrap_or(value);
        let token = match data
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(t) => t,
            None => return LoginOutcome::Failure("迅雷未返回 access_token".to_string()),
        };
        let mut cred = Credential::new(XUNLEI);
        cred.token = Some(token.to_string());
        cred.refresh_token = data
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        cred.expires_at = data
            .get("expires_in")
            .and_then(Value::as_i64)
            .filter(|s| *s > 0)
            .map(|secs| now_unix() + secs);
        let user_id = data
            .get("user_id")
            .or_else(|| data.get("sub"))
            .and_then(scalar);
        if let Some(id) = &user_id {
            cred.account_id = format!("xunlei:{id}");
            cred.masked_id = Some(mask_identity(id));
            cred.extra.insert("user_id".to_string(), id.clone());
        }
        cred.display_name = data
            .get("user_name")
            .or_else(|| data.get("nickname"))
            .or_else(|| data.get("name"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        LoginOutcome::Success(cred)
    }
}

/// `user_id` 可能是字符串或数字。
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

    /// mock 服务：`handler(url)` 返回 (HTTP 状态码, JSON body)。
    fn spawn_server<F>(handler: F) -> (String, std::thread::JoinHandle<()>)
    where
        F: Fn(&str) -> (u16, String) + Send + 'static,
    {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let (code, body) = handler(req.url());
                let resp = tiny_http::Response::from_string(body)
                    .with_status_code(code)
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    );
                let _ = req.respond(resp);
            }
        });
        (base, handle)
    }

    const CODE_JSON: &str = r#"{"device_code":"dc-1","user_code":"UC-1","expires_in":120,"interval":2,"verification_url":"https://xluser-ssl.xunlei.com/__/auth/device/?client_id=c","verification_uri_complete":"https://xluser-ssl.xunlei.com/__/auth/device/?client_id=c&user_code=UC-1","short_uri_complete":"https://xluser-ssl.xunlei.com/api/v1/reurl?code=x"}"#;
    const PENDING_JSON: &str = r#"{"error":"authorization_pending","error_code":4050,"error_description":"Precondition Required","details":[{"message":"Authorizing"}]}"#;
    const TOKEN_JSON: &str = r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":7200,"token_type":"Bearer","user_id":9,"user_name":"迅雷用户"}"#;

    #[test]
    fn mock_device_flow_ticket_then_pending_then_success() {
        let seq = Arc::new(AtomicUsize::new(0));
        let seq2 = seq.clone();
        let (base, _h) = spawn_server(move |url| {
            if url.contains("/auth/device/code") {
                (200, CODE_JSON.to_string())
            } else if url.contains("/auth/token") {
                if seq2.fetch_add(1, Ordering::SeqCst) == 0 {
                    (400, PENDING_JSON.to_string())
                } else {
                    (200, TOKEN_JSON.to_string())
                }
            } else {
                (404, r#"{"error":"not_found"}"#.to_string())
            }
        });

        let flow = XunleiLogin::with_base(test_client(), &base);
        let ticket = flow.start().unwrap();
        assert_eq!(ticket.kind, QrKind::DeviceCode);
        assert_eq!(ticket.content, "UC-1");
        assert!(ticket.image_url.contains("user_code=UC-1"));
        // 设备码型同样给出可扫二维码（内容为 complete 短链或验证页链接）。
        assert!(
            ticket.qr_image.starts_with("data:image/svg+xml;base64,"),
            "qr_image={}",
            ticket.qr_image
        );
        assert_eq!(ticket.expires_in, 120);
        assert_eq!(
            ticket.state.get("device_code").map(String::as_str),
            Some("dc-1")
        );

        assert_eq!(flow.poll(&ticket).unwrap(), LoginOutcome::Waiting);
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Success(cred) => {
                assert_eq!(cred.provider, "xunlei");
                assert_eq!(cred.account_id, "xunlei:9");
                assert_eq!(cred.token.as_deref(), Some("at-1"));
                assert_eq!(cred.refresh_token.as_deref(), Some("rt-1"));
                assert!(cred.expires_at.unwrap() > now_unix());
                assert_eq!(cred.display_name.as_deref(), Some("迅雷用户"));
                assert!(cred.masked_id.is_some());
            }
            other => panic!("期望 Success，实际 {other:?}"),
        }
    }

    #[test]
    fn mock_expired_and_denied() {
        let (base, _h) = spawn_server(|_url| {
            (
                400,
                r#"{"error":"expired_token","error_description":"Expired"}"#.to_string(),
            )
        });
        let flow = XunleiLogin::with_base(test_client(), &base);
        let mut ticket = QrTicket::new("t", QrKind::DeviceCode, "UC", "", 120);
        ticket.state.insert("device_code".to_string(), "dc".into());
        assert_eq!(flow.poll(&ticket).unwrap(), LoginOutcome::Expired);

        let (base, _h) = spawn_server(|_url| {
            (
                400,
                r#"{"error":"access_denied","error_description":"Denied"}"#.to_string(),
            )
        });
        let flow = XunleiLogin::with_base(test_client(), &base);
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Failure(msg) => assert!(msg.contains("拒绝"), "{msg}"),
            other => panic!("期望 Failure，实际 {other:?}"),
        }
    }

    #[test]
    fn slow_down_and_pending_are_waiting() {
        let (base, _h) = spawn_server(|_url| (400, r#"{"error":"slow_down"}"#.to_string()));
        let flow = XunleiLogin::with_base(test_client(), &base);
        let mut ticket = QrTicket::new("t", QrKind::DeviceCode, "UC", "", 120);
        ticket.state.insert("device_code".to_string(), "dc".into());
        assert_eq!(flow.poll(&ticket).unwrap(), LoginOutcome::Waiting);
    }

    #[test]
    fn poll_without_device_code_is_failure() {
        let (base, _h) = spawn_server(|_url| (200, TOKEN_JSON.to_string()));
        let flow = XunleiLogin::with_base(test_client(), &base);
        let ticket = QrTicket::new("t", QrKind::DeviceCode, "UC", "", 120);
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Failure(msg) => assert!(msg.contains("device_code"), "{msg}"),
            other => panic!("期望 Failure，实际 {other:?}"),
        }
    }

    #[test]
    fn start_rejects_non_200_and_bad_payload() {
        let (base, _h) = spawn_server(|_url| (500, r#"{"error":"boom"}"#.to_string()));
        let flow = XunleiLogin::with_base(test_client(), &base);
        assert!(flow.start().is_err());

        let (base, _h) = spawn_server(|_url| (200, r#"{"user_code":"only"}"#.to_string()));
        let flow = XunleiLogin::with_base(test_client(), &base);
        assert!(flow.start().is_err());
    }

    #[test]
    #[ignore = "live: 需要外网访问 xluser-ssl.xunlei.com（只申请设备码，不做确认）"]
    fn live_device_code_shape() {
        let flow = XunleiLogin::new(super::super::login_client().unwrap());
        let ticket = flow.start().unwrap();
        assert_eq!(ticket.kind, QrKind::DeviceCode);
        assert!(!ticket.content.is_empty());
        assert!(!ticket.state.get("device_code").unwrap().is_empty());
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Waiting => {}
            other => panic!("未确认时应为 Waiting，实际 {other:?}"),
        }
    }
}
