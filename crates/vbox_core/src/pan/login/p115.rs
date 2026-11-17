//! 115 网盘扫码登录（W13 D3）。
//!
//! 协议来源（2026-11-17 真站实测 + `p115client` 公开实现交叉验证）：
//!
//! 1. `GET {base}/api/1.0/web/1.0/token/`
//!    → `{state:1, code:0, data:{uid, time, sign, qrcode}}`；`qrcode` 缺省时用
//!    `https://115.com/scan/dg-{uid}` 补；
//! 2. 二维码图片 `GET {base}/api/1.0/web/1.0/qrcode?uid={uid}`（`image/png`）；
//! 3. 状态长轮询 `GET {base}/get/status/?uid=&time=&sign=&_=`（约 30s 返回一次）：
//!    `data.status`：0/缺省=等待、1=已扫码、2=已确认、-1=过期、-2=已取消；
//!    兼容 catvod 旧判定（`data.version` 存在且 `msg` 为空视为已确认）；
//! 4. 换 Cookie `POST {base}/app/1.0/web/1.0/login/qrcode/`（form `account={uid}`）
//!    → `data.cookie` 字典（`UID/CID/SEID/KID`），或回退读 `Set-Cookie`。
//!
//! 安全：Cookie 只进入 [`Credential`]（overlay/凭据文件），不打日志、不返回前端明文。

use std::collections::HashMap;

use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::http;

use super::{mask_identity, now_unix, Credential, LoginFlow, LoginOutcome, QrKind, QrTicket, P115};

/// 115 扫码 API 根。
pub const BASE: &str = "https://qrcodeapi.115.com";
/// 二维码有效期（115 网页端约 5 分钟；过期以状态接口 -1 为准）。
pub const TTL_SECS: u64 = 300;

const TOKEN_PATH: &str = "/api/1.0/web/1.0/token/";
const STATUS_PATH: &str = "/get/status/";
const LOGIN_QRCODE_PATH: &str = "/app/1.0/web/1.0/login/qrcode/";

/// 115 扫码登录流。
pub struct P115Login {
    client: reqwest::blocking::Client,
    base: String,
}

impl P115Login {
    /// 使用默认真站地址。
    pub fn new(client: reqwest::blocking::Client) -> Self {
        Self::with_base(client, BASE)
    }

    /// 指定 API 根（mock 测试用）。
    pub fn with_base(client: reqwest::blocking::Client, base: &str) -> Self {
        Self {
            client,
            base: base.trim_end_matches('/').to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn get_json(&self, path: &str, retries: u32) -> Result<Value> {
        let text = http::get_text(&self.client, &self.url(path), &HashMap::new(), retries)?;
        serde_json::from_str(&text)
            .map_err(|e| CoreError::Config(format!("115 登录响应不是 JSON: {e}")))
    }

    /// 拿到 status=2 后换 Cookie，组装 [`LoginOutcome::Success`]。
    fn finish(&self, uid: &str) -> Result<LoginOutcome> {
        let mut form = HashMap::new();
        form.insert("account".to_string(), uid.to_string());
        let (text, headers) = match http::post_form_text_with_headers(
            &self.client,
            &self.url(LOGIN_QRCODE_PATH),
            &HashMap::new(),
            &form,
        ) {
            Ok(v) => v,
            Err(e) => {
                return Ok(LoginOutcome::Failure(format!(
                    "115 换取登录 Cookie 失败: {e}"
                )))
            }
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => return Ok(LoginOutcome::Failure(format!("115 登录响应解析失败: {e}"))),
        };
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        let mut cookie = data.get("cookie").map(cookie_to_string).unwrap_or_default();
        if cookie.trim().is_empty() {
            cookie = cookie_from_headers(&headers);
        }
        if cookie.trim().is_empty() {
            let msg = value
                .get("message")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .unwrap_or("115 未返回登录 Cookie");
            return Ok(LoginOutcome::Failure(msg.to_string()));
        }

        let mut cred = Credential::new(P115);
        cred.account_id = format!("p115:{uid}");
        cred.cookie = Some(cookie);
        cred.masked_id = Some(mask_identity(uid));
        cred.display_name = data
            .get("user_name")
            .or_else(|| data.get("nickname"))
            .or_else(|| data.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string);
        cred.extra.insert("uid".to_string(), uid.to_string());
        Ok(LoginOutcome::Success(cred))
    }
}

impl LoginFlow for P115Login {
    fn provider(&self) -> &'static str {
        P115
    }

    fn start(&self) -> Result<QrTicket> {
        let value = self.get_json(TOKEN_PATH, 1)?;
        let data = value
            .get("data")
            .filter(|d| d.is_object())
            .ok_or_else(|| CoreError::Config(format!("115 未返回二维码 token: {value}")))?;
        let uid = json_scalar(data.get("uid"))
            .ok_or_else(|| CoreError::Config("115 token 缺少 uid".to_string()))?;
        let time = json_scalar(data.get("time")).unwrap_or_default();
        let sign = data
            .get("sign")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if sign.is_empty() {
            return Err(CoreError::Config("115 token 缺少 sign".to_string()));
        }
        let content = data
            .get("qrcode")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("https://115.com/scan/dg-{uid}"));

        let mut ticket = QrTicket::new(
            uid.clone(),
            QrKind::QrCode,
            content,
            format!("{}/api/1.0/web/1.0/qrcode?uid={uid}", self.base),
            TTL_SECS,
        );
        ticket.state.insert("uid".to_string(), uid);
        ticket.state.insert("time".to_string(), time);
        ticket.state.insert("sign".to_string(), sign);
        Ok(ticket)
    }

    fn poll(&self, ticket: &QrTicket) -> Result<LoginOutcome> {
        let uid = ticket.state.get("uid").cloned().unwrap_or_default();
        if uid.is_empty() {
            return Ok(LoginOutcome::Failure("115 票据缺少 uid".to_string()));
        }
        let time = ticket.state.get("time").cloned().unwrap_or_default();
        let sign = ticket.state.get("sign").cloned().unwrap_or_default();
        let path = format!(
            "{STATUS_PATH}?uid={uid}&time={time}&sign={sign}&_={}",
            now_unix()
        );
        // 长轮询超时/网络抖动都视为「暂无变化」，由票据有效期兜底。
        let value = match self.get_json(&path, 0) {
            Ok(v) => v,
            Err(_) => {
                return Ok(if ticket.is_expired() {
                    LoginOutcome::Expired
                } else {
                    LoginOutcome::Waiting
                })
            }
        };
        // 实测过期响应：`{"state":0,"code":40199002,"message":"key invalid","data":{}}`
        // （data 为空，没有 status 字段），遇到时直接按过期处理，否则会被当成 Waiting 一直轮询。
        let top_state = value.get("state").and_then(Value::as_i64).unwrap_or(1);
        let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = value.get("message").and_then(Value::as_str).unwrap_or("");
        if top_state == 0 || code == 40199002 || message.contains("key invalid") {
            return Ok(LoginOutcome::Expired);
        }

        let data = value.get("data").cloned().unwrap_or(Value::Null);

        // 兼容 catvod 旧判定：data.version 存在且 data.msg 为空 = 已确认登录。
        if data.get("version").is_some()
            && data
                .get("msg")
                .and_then(Value::as_str)
                .map(str::is_empty)
                .unwrap_or(false)
        {
            return self.finish(&uid);
        }

        match data.get("status").and_then(Value::as_i64) {
            None | Some(0) => Ok(LoginOutcome::Waiting),
            Some(1) => Ok(LoginOutcome::Scanned),
            Some(2) => self.finish(&uid),
            Some(-1) => Ok(LoginOutcome::Expired),
            Some(-2) => Ok(LoginOutcome::Failure("115 二维码已被取消".to_string())),
            Some(other) => Ok(LoginOutcome::Failure(format!("115 未知扫码状态: {other}"))),
        }
    }
}

/// `uid`/`time` 在 115 响应里可能是数字或字符串。
fn json_scalar(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 把 `data.cookie`（字典或字符串）转成 `k=v; k=v`。
///
/// 固定把 `UID/CID/SEID/KID` 排在前面（115 网页端顺序），其余键按字典序，
/// 保证同一响应生成稳定字符串。
fn cookie_to_string(cookie: &Value) -> String {
    match cookie {
        Value::Object(map) => {
            let preferred = ["UID", "CID", "SEID", "KID"];
            let mut pairs: Vec<(&str, String)> = map
                .iter()
                .filter_map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => return None,
                    };
                    (!val.is_empty()).then(|| (k.as_str(), val))
                })
                .collect();
            pairs.sort_by_key(|(k, _)| {
                let rank = preferred
                    .iter()
                    .position(|p| p == k)
                    .map(|i| i as i32)
                    .unwrap_or(preferred.len() as i32);
                (rank, k.to_string())
            });
            pairs
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ")
        }
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// 从响应头兜底取 `Set-Cookie`（每个 cookie 只保留 `NAME=VALUE` 段）。
fn cookie_from_headers(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .filter(|(name, _)| name == "set-cookie")
        .filter_map(|(_, value)| value.split(';').next())
        .map(str::trim)
        .filter(|pair| pair.contains('='))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rules;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn test_client() -> reqwest::blocking::Client {
        http::client_for(&Rules::default()).unwrap()
    }

    /// 起一个按 `handler(url)` 返回 JSON 的本地 mock 服务。
    fn spawn_json_server<F>(handler: F) -> (String, std::thread::JoinHandle<()>)
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = handler(req.url());
                let resp = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        (base, handle)
    }

    const TOKEN_JSON: &str = r#"{"state":1,"code":0,"data":{"uid":"uid-1","time":1700000000,"sign":"sig-1","qrcode":"https://115.com/scan/dg-uid-1"}}"#;
    const LOGIN_JSON: &str = r#"{"state":1,"code":0,"data":{"user_name":"测试用户","cookie":{"UID":"u1","CID":"c1","SEID":"s1","KID":"k1"}}}"#;

    #[test]
    fn mock_flow_ticket_shape_then_wait_scan_success() {
        let seq = Arc::new(AtomicUsize::new(0));
        let seq2 = seq.clone();
        let (base, _h) = spawn_json_server(move |url| {
            if url.contains("/token") {
                TOKEN_JSON.to_string()
            } else if url.contains("/get/status/") {
                let status = match seq2.fetch_add(1, Ordering::SeqCst) {
                    0 => r#"{"state":1,"code":0,"data":{"status":0}}"#,
                    1 => r#"{"state":1,"code":0,"data":{"status":1}}"#,
                    _ => r#"{"state":1,"code":0,"data":{"status":2}}"#,
                };
                status.to_string()
            } else if url.contains("/login/qrcode/") {
                LOGIN_JSON.to_string()
            } else {
                r#"{"state":0,"code":404,"message":"not found"}"#.to_string()
            }
        });

        let flow = P115Login::with_base(test_client(), &base);
        let ticket = flow.start().unwrap();
        assert_eq!(ticket.ticket_id, "uid-1");
        assert_eq!(ticket.kind, QrKind::QrCode);
        assert_eq!(ticket.content, "https://115.com/scan/dg-uid-1");
        assert!(ticket
            .image_url
            .ends_with("/api/1.0/web/1.0/qrcode?uid=uid-1"));
        assert_eq!(ticket.expires_in, TTL_SECS);
        assert_eq!(
            ticket.state.get("time").map(String::as_str),
            Some("1700000000")
        );
        assert_eq!(ticket.state.get("sign").map(String::as_str), Some("sig-1"));

        assert_eq!(flow.poll(&ticket).unwrap(), LoginOutcome::Waiting);
        assert_eq!(flow.poll(&ticket).unwrap(), LoginOutcome::Scanned);
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Success(cred) => {
                assert_eq!(cred.provider, "p115");
                assert_eq!(cred.account_id, "p115:uid-1");
                assert_eq!(
                    cred.cookie.as_deref(),
                    Some("UID=u1; CID=c1; SEID=s1; KID=k1")
                );
                assert_eq!(cred.display_name.as_deref(), Some("测试用户"));
                assert_eq!(cred.extra.get("uid").map(String::as_str), Some("uid-1"));
            }
            other => panic!("期望 Success，实际 {other:?}"),
        }
    }

    #[test]
    fn mock_status_expired_and_canceled() {
        let expired = r#"{"state":1,"code":0,"data":{"status":-1}}"#;
        let (base, _h) = spawn_json_server(move |_url| expired.to_string());
        let flow = P115Login::with_base(test_client(), &base);
        let ticket = QrTicket::new("uid-1", QrKind::QrCode, "c", "", TTL_SECS);
        // 直接给状态，跳过 start 的 token 请求。
        let mut ticket = ticket;
        ticket.state.insert("uid".to_string(), "uid-1".to_string());
        assert_eq!(flow.poll(&ticket).unwrap(), LoginOutcome::Expired);

        let canceled = r#"{"state":1,"code":0,"data":{"status":-2}}"#;
        let (base, _h) = spawn_json_server(move |_url| canceled.to_string());
        let flow = P115Login::with_base(test_client(), &base);
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Failure(msg) => assert!(msg.contains("取消"), "{msg}"),
            other => panic!("期望 Failure，实际 {other:?}"),
        }
    }

    #[test]
    fn mock_key_invalid_is_expired() {
        // 2026-11-17 真站实测的过期响应：data 为空，只有 state/code/message。
        let expired = r#"{"state":0,"code":40199002,"message":"key invalid","data":{}}"#;
        let (base, _h) = spawn_json_server(move |_url| expired.to_string());
        let flow = P115Login::with_base(test_client(), &base);
        let mut ticket = QrTicket::new("uid-1", QrKind::QrCode, "c", "", TTL_SECS);
        ticket.state.insert("uid".to_string(), "uid-1".to_string());
        assert_eq!(flow.poll(&ticket).unwrap(), LoginOutcome::Expired);
    }

    #[test]
    fn mock_version_confirm_fallback() {
        let confirmed = r#"{"state":1,"code":0,"data":{"version":"1.0","msg":""}}"#;
        let (base, _h) = spawn_json_server(move |url| {
            if url.contains("/login/qrcode/") {
                LOGIN_JSON.to_string()
            } else {
                confirmed.to_string()
            }
        });
        let flow = P115Login::with_base(test_client(), &base);
        let mut ticket = QrTicket::new("uid-1", QrKind::QrCode, "c", "", TTL_SECS);
        ticket.state.insert("uid".to_string(), "uid-1".to_string());
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Success(cred) => assert_eq!(cred.provider, "p115"),
            other => panic!("期望 Success，实际 {other:?}"),
        }
    }

    #[test]
    fn start_rejects_invalid_token_payload() {
        let (base, _h) = spawn_json_server(|_url| {
            r#"{"state":0,"code":10001,"message":"bad request"}"#.to_string()
        });
        let flow = P115Login::with_base(test_client(), &base);
        assert!(flow.start().is_err());

        let (base, _h) =
            spawn_json_server(|_url| r#"{"state":1,"code":0,"data":{"uid":"u"}}"#.to_string());
        let flow = P115Login::with_base(test_client(), &base);
        assert!(flow.start().is_err(), "缺少 sign 应失败");
    }

    #[test]
    fn poll_without_uid_is_failure() {
        let (base, _h) = spawn_json_server(|_url| TOKEN_JSON.to_string());
        let flow = P115Login::with_base(test_client(), &base);
        let ticket = QrTicket::new("t", QrKind::QrCode, "c", "", TTL_SECS);
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Failure(msg) => assert!(msg.contains("uid"), "{msg}"),
            other => panic!("期望 Failure，实际 {other:?}"),
        }
    }

    #[test]
    #[ignore = "live: 需要外网访问 qrcodeapi.115.com，且状态轮询会等 ~30s"]
    fn live_token_and_waiting_status() {
        let flow = P115Login::new(super::super::login_client().unwrap());
        let ticket = flow.start().unwrap();
        assert!(!ticket.ticket_id.is_empty());
        assert!(ticket.image_url.contains("/qrcode?uid="));
        assert!(!ticket.state.get("sign").unwrap().is_empty());
        match flow.poll(&ticket).unwrap() {
            LoginOutcome::Waiting | LoginOutcome::Scanned => {}
            other => panic!("未扫码时应为 Waiting，实际 {other:?}"),
        }
    }
}
