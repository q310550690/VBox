//! PikPak 网盘设备码登录（W13 D3，OAuth 2.0 Device Authorization Grant）。
//!
//! 协议来源（2026-11-18 真站实测）：
//!
//! 1. `POST {auth_base}auth/device/code`，JSON `{"client_id":..,"scope":..}`，
//!    头 `X-Client-Id: <client_id>` → `{device_code, user_code, expires_in:300,
//!    interval:2, verification_url, verification_uri_complete}`；
//! 2. 扫码页：`client_id` 用 `YUMx5nI8ZU8Ap8pm` 时 `verification_uri_complete` 为
//!    `https://mypikpak.com/drive/qr-login?client_id=..&scope=..&user_code=..`，
//!    用 PikPak App 扫码或输入 `user_code` 确认；
//! 3. `POST {auth_base}auth/token`，JSON
//!    `{"grant_type":"urn:ietf:params:oauth:grant-type:device_code",
//!      "device_code":.., "client_id":..}`：
//!    - 400 `authorization_pending`（`error_code:4050`）→ 等待；
//!    - 400 `slow_down`/HTTP 428 → 等待；
//!    - 400 `expired_token` → 过期；
//!    - 400 `access_denied` → 用户拒绝；
//!    - 200 → `access_token` + `refresh_token` + `expires_in`；
//! 4. 成功后 `GET {auth_base}user/me` 取昵称（失败不影响登录）。
//!
//! 说明：drive 接口（`api-drive.mypikpak.com`）只校验 Bearer token、不校验
//! `X-Client-Id`（2026-11-18 实测：无该头同样能通过鉴权校验），因此登录端使用
//! 扫码版 client_id 不会影响个人盘浏览。
//!
//! 安全：access/refresh token 只进入 [`Credential`]，不打日志、不返回前端明文。

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::pan::xunlei::PIKPAK_AUTH_BASE;

use super::{
    mask_identity, now_unix, qr_data_url, Credential, LoginFlow, LoginOutcome, QrKind, QrTicket,
    LOGIN_UA, PIKPAK,
};

/// 扫码版 client_id：`verification_uri_complete` 指向官方扫码页
/// `https://mypikpak.com/drive/qr-login`。
pub const PIKPAK_QR_CLIENT_ID: &str = "YUMx5nI8ZU8Ap8pm";

/// 设备码 / token / 用户信息接口路径（相对 `PIKPAK_AUTH_BASE`）。
const DEVICE_CODE_PATH: &str = "auth/device/code";
const TOKEN_PATH: &str = "auth/token";
const USER_ME_PATH: &str = "user/me";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// 登录授权范围（与官方扫码页一致）。
const SCOPE: &str = "pan user offline account cloud_download files:write files:read \
files:purge files:manage invite share";
/// 兜底验证页。
const FALLBACK_VERIFY: &str = "https://mypikpak.com/drive/login?redirect=/all";

/// PikPak 设备码登录流。
pub struct PikPakLogin {
    client: reqwest::blocking::Client,
    base: String,
    client_id: String,
}

impl PikPakLogin {
    /// 使用默认真站地址与扫码版 client_id。
    pub fn new(client: reqwest::blocking::Client) -> Self {
        Self::with_base(client, PIKPAK_AUTH_BASE)
    }

    /// 指定 API 根（mock 测试用）。
    pub fn with_base(client: reqwest::blocking::Client, base: &str) -> Self {
        Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            client_id: PIKPAK_QR_CLIENT_ID.to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    /// 发 JSON POST 并返回（状态码, 响应文本）；非 2xx 也要读 body 以识别 OAuth 错误码。
    fn post_json(&self, path: &str, body: &Value, device_id: &str) -> Result<(u16, String)> {
        let resp = self
            .client
            .post(self.url(path))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", LOGIN_UA)
            .header("X-Client-Id", &self.client_id)
            .header("X-Device-Id", device_id)
            .body(body.to_string())
            .send()
            .map_err(CoreError::Http)?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().map_err(CoreError::Http)?;
        Ok((status, String::from_utf8_lossy(&bytes).to_string()))
    }

    fn device_id(&self) -> String {
        // 设备标识只需在一次登录会话内稳定；用时间戳拼接进程号即可，无需额外依赖。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("vbox-{:x}-{}", nanos, std::process::id())
    }
}

impl LoginFlow for PikPakLogin {
    fn provider(&self) -> &'static str {
        PIKPAK
    }

    fn start(&self) -> Result<QrTicket> {
        let device_id = self.device_id();
        let body = json!({ "client_id": self.client_id, "scope": SCOPE });
        let (status, text) = self.post_json(DEVICE_CODE_PATH, &body, &device_id)?;
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status != 200 {
            return Err(CoreError::Api {
                code: status as i64,
                msg: format!("PikPak 设备码申请失败: {}", error_text(&value)),
            });
        }
        let device_code = value
            .get("device_code")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Api {
                code: status as i64,
                msg: "PikPak 设备码响应缺少 device_code".to_string(),
            })?
            .to_string();
        let user_code = value
            .get("user_code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let expires_in = value
            .get("expires_in")
            .and_then(Value::as_u64)
            .filter(|s| *s > 0)
            .unwrap_or(300)
            .clamp(30, 900);
        let verify_url = value
            .get("verification_uri_complete")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| value.get("verification_url").and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .unwrap_or(FALLBACK_VERIFY)
            .to_string();

        let mut ticket = QrTicket::new("", QrKind::DeviceCode, user_code, &verify_url, expires_in);
        ticket.qr_image = qr_data_url(&verify_url);
        ticket.state.insert("device_code".to_string(), device_code);
        ticket.state.insert("device_id".to_string(), device_id);
        Ok(ticket)
    }

    fn poll(&self, ticket: &QrTicket) -> Result<LoginOutcome> {
        let device_code = ticket
            .state
            .get("device_code")
            .map(String::as_str)
            .unwrap_or_default()
            .to_string();
        if device_code.is_empty() {
            return Ok(LoginOutcome::Failure(
                "PikPak 票据缺少 device_code".to_string(),
            ));
        }
        let device_id = ticket
            .state
            .get("device_id")
            .cloned()
            .unwrap_or_else(|| self.device_id());
        let body = json!({
            "grant_type": DEVICE_GRANT,
            "device_code": device_code,
            "client_id": self.client_id,
        });
        // 网络抖动视为「暂无变化」，由票据有效期兜底。
        let (status, text) = match self.post_json(TOKEN_PATH, &body, &device_id) {
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
            return Ok(self.success_from_value(&value, &device_id));
        }

        let error = value.get("error").and_then(Value::as_str).unwrap_or("");
        match error {
            "authorization_pending" | "slow_down" => Ok(if ticket.is_expired() {
                LoginOutcome::Expired
            } else {
                LoginOutcome::Waiting
            }),
            "expired_token" => Ok(LoginOutcome::Expired),
            "access_denied" => Ok(LoginOutcome::Failure("PikPak 授权已被拒绝".to_string())),
            _ if status == 428 => Ok(LoginOutcome::Waiting),
            _ => Ok(LoginOutcome::Failure(format!(
                "PikPak 登录失败: HTTP {status} {}",
                error_text(&value)
            ))),
        }
    }
}

impl PikPakLogin {
    fn success_from_value(&self, value: &Value, device_id: &str) -> LoginOutcome {
        let data = value.get("data").filter(|d| d.is_object()).unwrap_or(value);
        let token = match data
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(t) => t,
            None => return LoginOutcome::Failure("PikPak 未返回 access_token".to_string()),
        };
        let mut cred = Credential::new(PIKPAK);
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
            .get("sub")
            .or_else(|| data.get("user_id"))
            .and_then(scalar);
        if let Some(id) = &user_id {
            cred.account_id = format!("pikpak:{id}");
            cred.masked_id = Some(mask_identity(id));
            cred.extra.insert("user_id".to_string(), id.clone());
        }
        if let Some(name) = self.profile_name(token, device_id) {
            cred.display_name = Some(name);
        }
        LoginOutcome::Success(cred)
    }

    /// 成功后取用户昵称；失败（网络/权限）不影响登录结果。
    fn profile_name(&self, token: &str, device_id: &str) -> Option<String> {
        let resp = self
            .client
            .get(self.url(USER_ME_PATH))
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json")
            .header("User-Agent", LOGIN_UA)
            .header("X-Client-Id", &self.client_id)
            .header("X-Device-Id", device_id)
            .send()
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let value: Value = resp.json().ok()?;
        ["name", "nickname", "email"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
}

fn error_text(value: &Value) -> String {
    let err = value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("未知错误");
    let desc = value
        .get("error_description")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    let code = value
        .get("error_code")
        .and_then(Value::as_i64)
        .map(|c| format!(" ({c})"))
        .unwrap_or_default();
    if desc.is_empty() {
        format!("{err}{code}")
    } else {
        format!("{err}{code}: {desc}")
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
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn test_client() -> reqwest::blocking::Client {
        crate::http::client_for(&Rules::default()).expect("client")
    }

    /// 起一个只处理固定次数的 mock 服务器；返回（基址, 命中计数）。
    fn spawn_server(
        responses: Vec<(u16, String)>,
    ) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let port = server.server_addr().to_ip().expect("addr").port();
        let counter = Arc::new(AtomicUsize::new(0));
        let hits = Arc::clone(&counter);
        let handle = std::thread::spawn(move || {
            for (status, body) in responses {
                let request = match server.recv() {
                    Ok(r) => r,
                    Err(_) => break,
                };
                hits.fetch_add(1, Ordering::SeqCst);
                let mut request = request;
                let mut buf = String::new();
                let _ = request.as_reader().read_to_string(&mut buf);
                let response = tiny_http::Response::from_string(body)
                    .with_status_code(tiny_http::StatusCode(status));
                let _ = request.respond(response);
            }
        });
        (format!("http://127.0.0.1:{port}/v1/"), counter, handle)
    }

    #[test]
    fn start_builds_qr_login_ticket_and_polls_to_success() {
        let (base, hits, handle) = spawn_server(vec![
            (
                200,
                r#"{"device_code":"dc-1","user_code":"uc-1","expires_in":300,"interval":2,
                    "verification_url":"https://mypikpak.com/drive/qr-login?client_id=c",
                    "verification_uri_complete":"https://mypikpak.com/drive/qr-login?client_id=c&user_code=uc-1"}"#
                    .to_string(),
            ),
            (
                400,
                r#"{"error":"authorization_pending","error_code":4050,"error_description":"Precondition Required"}"#
                    .to_string(),
            ),
            (
                200,
                r#"{"access_token":"at","refresh_token":"rt","expires_in":7200,"sub":"u-1"}"#
                    .to_string(),
            ),
            (200, r#"{"name":"pikpak-user"}"#.to_string()),
        ]);
        let login = PikPakLogin::with_base(test_client(), &base);

        let ticket = login.start().expect("start");
        assert_eq!(ticket.kind, QrKind::DeviceCode);
        assert_eq!(ticket.content, "uc-1");
        assert!(ticket.image_url.contains("mypikpak.com/drive/qr-login"));
        assert!(ticket.qr_image.starts_with("data:image/svg+xml;base64,"));
        assert_eq!(ticket.expires_in, 300);
        assert_eq!(
            ticket.state.get("device_code").map(String::as_str),
            Some("dc-1")
        );

        assert_eq!(login.poll(&ticket).expect("poll 1"), LoginOutcome::Waiting);
        let outcome = login.poll(&ticket).expect("poll 2");
        match outcome {
            LoginOutcome::Success(cred) => {
                assert_eq!(cred.provider, PIKPAK);
                assert_eq!(cred.token.as_deref(), Some("at"));
                assert_eq!(cred.refresh_token.as_deref(), Some("rt"));
                assert_eq!(cred.account_id, "pikpak:u-1");
                assert_eq!(cred.display_name.as_deref(), Some("pikpak-user"));
                assert!(cred.expires_at.unwrap_or(0) > now_unix());
            }
            other => panic!("期望成功，实际 {other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 4);
        let _ = handle.join();
    }

    #[test]
    fn expired_ticket_maps_to_expired() {
        let (base, _hits, handle) = spawn_server(vec![(
            400,
            r#"{"error":"expired_token","error_description":"expired"}"#.to_string(),
        )]);
        let login = PikPakLogin::with_base(test_client(), &base);
        let mut ticket = QrTicket::new("", QrKind::DeviceCode, "uc", "https://x", 60);
        ticket
            .state
            .insert("device_code".to_string(), "dc".to_string());
        assert_eq!(login.poll(&ticket).expect("poll"), LoginOutcome::Expired);
        let _ = handle.join();
    }

    #[test]
    fn missing_device_code_fails_without_network() {
        let login = PikPakLogin::with_base(test_client(), "http://127.0.0.1:9/v1/");
        let ticket = QrTicket::new("", QrKind::DeviceCode, "uc", "https://x", 60);
        match login.poll(&ticket).expect("poll") {
            LoginOutcome::Failure(msg) => assert!(msg.contains("device_code")),
            other => panic!("期望失败，实际 {other:?}"),
        }
    }

    #[test]
    fn start_reports_error_when_server_rejects() {
        let (base, _hits, handle) = spawn_server(vec![(
            400,
            r#"{"error":"invalid_client","error_code":4126,"error_description":"client not found"}"#
                .to_string(),
        )]);
        let login = PikPakLogin::with_base(test_client(), &base);
        let err = login.start().expect_err("应报错");
        let text = format!("{err}");
        assert!(text.contains("invalid_client"), "实际: {text}");
        let _ = handle.join();
    }

    /// 真站联调（默认忽略）：需要可访问 `user.mypikpak.net` 的网络。
    #[test]
    #[ignore = "live: 需要可访问 user.mypikpak.net"]
    fn live_device_code_returns_qr_login_page() {
        let login = PikPakLogin::new(test_client());
        let ticket = login.start().expect("live start");
        assert_eq!(ticket.kind, QrKind::DeviceCode);
        assert!(!ticket.content.is_empty());
        assert!(
            ticket.image_url.contains("mypikpak.com/drive/qr-login"),
            "实际: {}",
            ticket.image_url
        );
        assert!(ticket.qr_image.starts_with("data:image/svg+xml;base64,"));
        // 未扫码时应为等待（而非失败）。
        let outcome = login.poll(&ticket).expect("live poll");
        assert!(
            matches!(outcome, LoginOutcome::Waiting | LoginOutcome::Expired),
            "实际: {outcome:?}"
        );
    }

    #[allow(dead_code)]
    fn _assert_send_sync() {
        fn f<T: Send + Sync>() {}
        f::<PikPakLogin>();
    }
}
