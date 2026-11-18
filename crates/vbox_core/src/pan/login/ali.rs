//! 阿里云盘扫码登录（W14 补齐：passport 「havana」二维码协议）。
//!
//! 协议来源（2026-11-18 真站复核 `passport.aliyundrive.com` 可达，与网页登录弹窗同一套）：
//!
//! 1. `GET {passport}/newlogin/qrcode/generate.do?appName=aliyun_drive&fromSite=52&appEntrance=web…`
//!    → `{content:{data:{t, ck, codeContent}}}`：`codeContent` 是要编码进二维码的
//!    `qrcodeCheck.htm?lgToken=…` 链接，`ck` / `t` 是后续轮询键；
//! 2. 二维码内容本地渲染成 SVG（桌面端不依赖外网二维码服务）；
//! 3. `POST {passport}/newlogin/qrcode/query.do?…`（form `ck=<ck>&t=<t>`）
//!    → `content.data.qrCodeStatus`：`NEW` 等待扫码 → `SCANED` 已扫待确认 →
//!    `CONFIRMED` 已确认（`bizExt` 为 base64 JSON，内含 `pds_login_result`）→ `EXPIRED`；
//! 4. 确认后解 `bizExt` 取 `pds_login_result`：
//!    `access_token` / `refresh_token` / `expires_in` / `user_id` / `nick_name` /
//!    `default_drive_id` / `avatar`；
//! 5. 凭据写入 [`Credential`]：`token = access_token`、`refresh_token`、
//!    `expires_at = now + expires_in`，账号主键 `ali:<user_id>`。
//!
//! 安全：token 只进入 [`Credential`]，不打日志、不返回前端明文。

use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use serde_json::Value;

use crate::error::{CoreError, Result};

use super::{
    mask_identity, now_unix, qr_data_url, Credential, LoginFlow, LoginOutcome, QrKind, QrTicket,
    ALI,
};

/// passport 根（阿里云盘网页登录域）。
pub const PASSPORT_BASE: &str = "https://passport.aliyundrive.com";
/// 二维码有效期（阿里网页端 120s，这里留 180s 余量）。
pub const TTL_SECS: u64 = 180;

const GENERATE_PATH: &str = "/newlogin/qrcode/generate.do";
const QUERY_PATH: &str = "/newlogin/qrcode/query.do";
/// generate / query 共用查询参数（与网页登录弹窗一致）。
const QR_QUERY: &str = "appName=aliyun_drive&fromSite=52&appEntrance=web&isMobile=false&lang=zh_CN&returnUrl=&navlanguage=zh-CN&navPlatform=Win32";

/// 阿里云盘扫码登录流。
pub struct AliLogin {
    client: reqwest::blocking::Client,
    passport_base: String,
}

impl AliLogin {
    /// 使用默认真站地址。
    pub fn new(client: reqwest::blocking::Client) -> Self {
        Self::with_base(client, PASSPORT_BASE)
    }

    /// 指定 passport 根（mock 测试用）。
    pub fn with_base(client: reqwest::blocking::Client, passport_base: &str) -> Self {
        Self {
            client,
            passport_base: passport_base.trim_end_matches('/').to_string(),
        }
    }

    fn get_json(&self, url: &str) -> Result<Value> {
        let resp = self.client.get(url).send().map_err(CoreError::Http)?;
        if !resp.status().is_success() {
            return Err(CoreError::Config(format!(
                "阿里云盘登录接口返回 HTTP {}: {url}",
                resp.status()
            )));
        }
        let text = resp.text().map_err(CoreError::Http)?;
        parse_havana(&text)
    }

    /// 轮询二维码状态；确认后把 `bizExt` 解成凭据。
    fn query(&self, ticket: &QrTicket) -> Result<LoginOutcome> {
        let ck = ticket.state.get("ck").cloned().unwrap_or_default();
        let t = ticket.state.get("t").cloned().unwrap_or_default();
        if ck.is_empty() || t.is_empty() {
            return Ok(LoginOutcome::Failure("阿里云盘票据缺少 ck/t".to_string()));
        }
        let url = format!("{}{QUERY_PATH}?{QR_QUERY}", self.passport_base);
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Referer", "https://www.alipan.com/")
            .body(format!("ck={ck}&t={t}"))
            .send()
            .map_err(CoreError::Http)?;
        if !resp.status().is_success() {
            return Ok(if ticket.is_expired() {
                LoginOutcome::Expired
            } else {
                LoginOutcome::Waiting
            });
        }
        let text = resp.text().map_err(CoreError::Http)?;
        let value = match parse_havana(&text) {
            Ok(v) => v,
            Err(_) => {
                return Ok(if ticket.is_expired() {
                    LoginOutcome::Expired
                } else {
                    LoginOutcome::Waiting
                })
            }
        };
        let data = value
            .get("content")
            .and_then(|c| c.get("data"))
            .cloned()
            .unwrap_or(Value::Null);
        let status = data
            .get("qrCodeStatus")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match status {
            "NEW" => Ok(if ticket.is_expired() {
                LoginOutcome::Expired
            } else {
                LoginOutcome::Waiting
            }),
            "SCANED" => Ok(LoginOutcome::Scanned),
            "EXPIRED" | "CANCELED" => Ok(LoginOutcome::Expired),
            "CONFIRMED" => {
                let Some(biz_ext) = data.get("bizExt").and_then(Value::as_str) else {
                    return Ok(LoginOutcome::Failure("阿里云盘登录缺少 bizExt".to_string()));
                };
                match decode_biz_ext(biz_ext) {
                    Ok(cred) => Ok(LoginOutcome::Success(cred)),
                    Err(e) => Ok(LoginOutcome::Failure(e.to_string())),
                }
            }
            other => Ok(if ticket.is_expired() {
                LoginOutcome::Expired
            } else {
                let _ = other;
                LoginOutcome::Waiting
            }),
        }
    }
}

impl LoginFlow for AliLogin {
    fn provider(&self) -> &'static str {
        ALI
    }

    fn start(&self) -> Result<QrTicket> {
        let url = format!("{}{GENERATE_PATH}?{QR_QUERY}", self.passport_base);
        let value = self.get_json(&url)?;
        let data = value
            .get("content")
            .and_then(|c| c.get("data"))
            .ok_or_else(|| CoreError::Config("阿里云盘二维码响应缺少 data".to_string()))?;
        let content = data
            .get("codeContent")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Config("阿里云盘二维码响应缺少 codeContent".to_string()))?;
        let ck = data
            .get("ck")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Config("阿里云盘二维码响应缺少 ck".to_string()))?;
        let t = data
            .get("t")
            .and_then(scalar)
            .ok_or_else(|| CoreError::Config("阿里云盘二维码响应缺少 t".to_string()))?;
        // 阿里同样没有稳定的二维码图片地址：本地把 codeContent 渲染成二维码。
        let image_url = qr_data_url(content);
        let mut ticket = QrTicket::new("", QrKind::QrCode, content, image_url, TTL_SECS);
        ticket.state.insert("ck".to_string(), ck.to_string());
        ticket.state.insert("t".to_string(), t);
        Ok(ticket)
    }

    fn poll(&self, ticket: &QrTicket) -> Result<LoginOutcome> {
        self.query(ticket)
    }
}

/// 解 `bizExt`（base64 编码的 JSON）里的 `pds_login_result`。
fn decode_biz_ext(raw: &str) -> Result<Credential> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(raw.trim()))
        .map_err(|e| CoreError::Config(format!("阿里云盘 bizExt 不是合法 base64: {e}")))?;
    let text = String::from_utf8(bytes)
        .map_err(|e| CoreError::Config(format!("阿里云盘 bizExt 不是 UTF-8: {e}")))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| CoreError::Config(format!("阿里云盘 bizExt 不是合法 JSON: {e}")))?;
    let result = value
        .get("pds_login_result")
        .or_else(|| value.get("pdsLoginResult"))
        .cloned()
        .unwrap_or(Value::Null);
    let access_token = result
        .get("access_token")
        .or_else(|| result.get("accessToken"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::Config("阿里云盘登录结果缺少 access_token".to_string()))?;
    let expires_in = result
        .get("expires_in")
        .or_else(|| result.get("expiresIn"))
        .and_then(Value::as_i64)
        .filter(|v| *v > 0)
        .unwrap_or(7200);
    let user_id = result
        .get("user_id")
        .or_else(|| result.get("userId"))
        .and_then(scalar);
    let nick = result
        .get("nick_name")
        .or_else(|| result.get("nickName"))
        .or_else(|| result.get("user_name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut cred = Credential::new(ALI);
    cred.token = Some(access_token.to_string());
    cred.refresh_token = result
        .get("refresh_token")
        .or_else(|| result.get("refreshToken"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    cred.expires_at = Some(now_unix() + expires_in);
    cred.display_name = nick;
    if let Some(id) = &user_id {
        cred.account_id = format!("{ALI}:{id}");
        cred.masked_id = Some(mask_identity(id));
        cred.extra.insert("user_id".to_string(), id.clone());
    }
    if let Some(drive) = result
        .get("default_drive_id")
        .or_else(|| result.get("defaultDriveId"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        cred.extra
            .insert("default_drive_id".to_string(), drive.to_string());
    }
    // 阿里云盘接口同时接受 Bearer token 与 Cookie：优先 token，Cookie 留空避免泄露整段会话。
    Ok(cred)
}

/// 解析 havana 风格响应（`{content:{data:…}, hasError}`）。
fn parse_havana(text: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| CoreError::Config(format!("阿里云盘登录响应不是 JSON: {e}")))?;
    if value
        .get("hasError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let msg = value
            .get("content")
            .and_then(|c| c.get("data"))
            .and_then(|d| d.get("errorMsg"))
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        return Err(CoreError::Config(format!("阿里云盘登录失败: {msg}")));
    }
    Ok(value)
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 生成本地票据 id 的随机片段（宿主另会覆盖 `ticket_id`）。
fn _seq_hint() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
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
        F: Fn(&str, &str) -> (u16, String) + Send + 'static,
    {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let url = req.url().to_string();
                let mut body = String::new();
                use std::io::Read as _;
                let _ = req.as_reader().read_to_string(&mut body);
                let (code, payload) = handler(&url, &body);
                let _ =
                    req.respond(tiny_http::Response::from_string(payload).with_status_code(code));
            }
        });
        (base, handle)
    }

    fn biz_ext() -> String {
        let inner = serde_json::json!({
            "pds_login_result": {
                "access_token": "ali-token-1",
                "refresh_token": "ali-refresh-1",
                "expires_in": 7200,
                "user_id": "u-42",
                "nick_name": "阿里测试",
                "default_drive_id": "drive-1"
            }
        });
        base64::engine::general_purpose::STANDARD.encode(inner.to_string().as_bytes())
    }

    #[test]
    fn start_builds_ali_qr_and_poll_states() {
        let (base, _handle) = spawn_server(move |url, _body| {
            if url.contains("generate.do") {
                (
                    200,
                    serde_json::json!({
                        "content": {"data": {
                            "t": 1794994982785i64,
                            "ck": "ck-1",
                            "codeContent": "https://passport.aliyundrive.com/qrcodeCheck.htm?lgToken=abc"
                        }},
                        "hasError": false
                    })
                    .to_string(),
                )
            } else if url.contains("query.do") {
                (
                    200,
                    serde_json::json!({
                        "content": {"data": {"qrCodeStatus": "NEW", "resultCode": 100}},
                        "hasError": false
                    })
                    .to_string(),
                )
            } else {
                (404, "{}".to_string())
            }
        });
        let flow = AliLogin::with_base(test_client(), &base);
        let ticket = flow.start().unwrap();
        assert_eq!(ticket.kind, QrKind::QrCode);
        assert!(ticket.content.contains("qrcodeCheck.htm"));
        assert!(ticket.image_url.starts_with("data:image/svg+xml;base64,"));
        assert_eq!(ticket.state.get("ck").map(String::as_str), Some("ck-1"));
        assert_eq!(
            ticket.state.get("t").map(String::as_str),
            Some("1794994982785")
        );
        assert!(matches!(flow.poll(&ticket).unwrap(), LoginOutcome::Waiting));
    }

    #[test]
    fn confirmed_state_decodes_credential() {
        let ext = biz_ext();
        let (base, _handle) = spawn_server(move |url, _body| {
            if url.contains("generate.do") {
                (
                    200,
                    serde_json::json!({"content": {"data": {"t": 1, "ck": "c", "codeContent": "https://x/qrcodeCheck.htm?lgToken=1"}}, "hasError": false}).to_string(),
                )
            } else {
                (
                    200,
                    serde_json::json!({"content": {"data": {"qrCodeStatus": "CONFIRMED", "bizExt": ext, "resultCode": 100}}, "hasError": false}).to_string(),
                )
            }
        });
        let flow = AliLogin::with_base(test_client(), &base);
        let ticket = flow.start().unwrap();
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Success(cred) => {
                assert_eq!(cred.provider, ALI);
                assert_eq!(cred.token.as_deref(), Some("ali-token-1"));
                assert_eq!(cred.refresh_token.as_deref(), Some("ali-refresh-1"));
                assert_eq!(cred.account_id, "ali:u-42");
                assert_eq!(cred.display_name.as_deref(), Some("阿里测试"));
                assert_eq!(
                    cred.extra.get("default_drive_id").map(String::as_str),
                    Some("drive-1")
                );
                assert!(cred.expires_at.unwrap() > now_unix());
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[test]
    fn scanned_and_expired_states_are_reported() {
        let (base, _handle) = spawn_server(move |url, _body| {
            if url.contains("generate.do") {
                (200, serde_json::json!({"content": {"data": {"t": 1, "ck": "c", "codeContent": "https://x/qrcodeCheck.htm?lgToken=1"}}, "hasError": false}).to_string())
            } else if url.contains("gen-bad") {
                (200, "{}".to_string())
            } else {
                (200, serde_json::json!({"content": {"data": {"qrCodeStatus": "SCANED"}}, "hasError": false}).to_string())
            }
        });
        let flow = AliLogin::with_base(test_client(), &base);
        let ticket = flow.start().unwrap();
        assert!(matches!(flow.poll(&ticket).unwrap(), LoginOutcome::Scanned));
    }

    /// 真站冒烟：需要能访问 passport.aliyundrive.com 的网络。
    #[test]
    #[ignore = "live: 需要可访问 passport.aliyundrive.com 的网络"]
    fn live_passport_returns_scan_content() {
        let flow = AliLogin::new(super::super::login_client().unwrap());
        let ticket = flow.start().unwrap();
        assert!(
            ticket.content.contains("qrcodeCheck.htm"),
            "content={}",
            ticket.content
        );
        assert!(
            ticket.image_url.starts_with("data:image/svg+xml;base64,"),
            "image_url 应为本地渲染的二维码"
        );
        assert!(ticket.state.contains_key("ck") && ticket.state.contains_key("t"));
        assert!(matches!(
            flow.poll(&ticket).unwrap(),
            LoginOutcome::Waiting | LoginOutcome::Scanned
        ));
    }

    #[test]
    fn rejects_payload_without_code_content() {
        let (base, _handle) = spawn_server(|url, _body| {
            if url.contains("generate.do") {
                (200, serde_json::json!({"content": {"data": {"t": 1, "ck": "c"}}, "hasError": false}).to_string())
            } else {
                (200, "{}".to_string())
            }
        });
        let flow = AliLogin::with_base(test_client(), &base);
        let err = flow.start().unwrap_err().to_string();
        assert!(err.contains("codeContent"), "{err}");
    }
}
