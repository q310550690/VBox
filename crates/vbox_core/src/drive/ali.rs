//! 阿里云盘个人盘只读（W14 P2+，token 模式）。
//!
//! 与分享通道（`pan/ali.rs`，Cookie + `x-share-token`）不同，个人盘走 OAuth
//! `Authorization: Bearer <access_token>`：
//! - `POST {base}v2/user/get` `{}` → `default_drive_id`
//! - `POST {base}adrive/v3/file/list` `{drive_id,parent_file_id,limit,order_by,order_direction}` → `items[]`
//! - `POST {base}v2/file/get_download_url` `{drive_id,file_id,expire_sec}` → `url`
//!
//! 阿里接口每次请求都要 `drive_id`，因此条目 id 编码为 `<drive_id>|<file_id>`，
//! 前端只把它当不透明字符串回传。
//!
//! 安全边界：token 只在内存中使用，不落盘、不打日志、不返回错误明文。

use serde_json::{json, Value};

use super::DriveEntry;
use crate::error::{CoreError, Result};

/// 条目 id 中 drive_id 与 file_id 的分隔符。
pub const ID_SEP: char = '|';

fn post(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
    path: &str,
    body: &Value,
) -> Result<Value> {
    let url = format!("{base}{path}");
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {token}"))
        .header("x-canary", crate::pan::ali::X_CANARY)
        .header("referer", crate::pan::ali::REFERER)
        .json(body)
        .send()
        .map_err(CoreError::from)?;
    let status = resp.status();
    let text = resp.text().map_err(CoreError::from)?;
    if !status.is_success() {
        return Err(CoreError::Api {
            code: status.as_u16() as i64,
            msg: text.chars().take(160).collect(),
        });
    }
    serde_json::from_str(&text).map_err(|e| CoreError::Json {
        path: "ali".into(),
        msg: e.to_string(),
    })
}

fn default_drive_id(client: &reqwest::blocking::Client, base: &str, token: &str) -> Result<String> {
    let value = post(client, base, token, "v2/user/get", &json!({}))?;
    value
        .get("default_drive_id")
        .or_else(|| value.get("data").and_then(|d| d.get("default_drive_id")))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CoreError::Config("阿里云盘未返回 default_drive_id：请重新登录".into()))
}

/// 列一页目录（`parent` 为空时为根目录）。
pub fn list(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
    parent: Option<&str>,
) -> Result<Vec<DriveEntry>> {
    let (drive_id, parent_file_id) = match parent.and_then(parse_id) {
        Some((drive_id, file_id)) => (drive_id, file_id),
        None => (default_drive_id(client, base, token)?, "root".to_string()),
    };
    let body = json!({
        "drive_id": drive_id,
        "parent_file_id": parent_file_id,
        "limit": 200,
        "order_by": "name",
        "order_direction": "ASC",
    });
    let value = post(client, base, token, "adrive/v3/file/list", &body)?;
    Ok(parse_entries(&value, &drive_id))
}

/// 取个人盘文件直链。
pub fn download_url(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
    entry_id: &str,
) -> Result<String> {
    let (drive_id, file_id) = parse_id(entry_id).ok_or_else(|| {
        CoreError::Config("阿里云盘条目 id 无效（应为 <drive_id>|<file_id>）".into())
    })?;
    let body = json!({
        "drive_id": drive_id,
        "file_id": file_id,
        "expire_sec": 115200,
    });
    let value = post(client, base, token, "v2/file/get_download_url", &body)?;
    value
        .get("url")
        .or_else(|| value.get("data").and_then(|d| d.get("url")))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CoreError::Config("阿里云盘未返回下载直链（文件可能已失效）".into()))
}

/// 解析 `<drive_id>|<file_id>`。
pub fn parse_id(raw: &str) -> Option<(String, String)> {
    let (drive_id, file_id) = raw.split_once(ID_SEP)?;
    let drive_id = drive_id.trim();
    let file_id = file_id.trim();
    if drive_id.is_empty() || file_id.is_empty() {
        return None;
    }
    Some((drive_id.to_string(), file_id.to_string()))
}

fn parse_entries(value: &Value, drive_id: &str) -> Vec<DriveEntry> {
    let items = value
        .get("items")
        .or_else(|| value.get("data").and_then(|d| d.get("items")))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|item| {
            let file_id = item.get("file_id").and_then(Value::as_str)?;
            let name = item.get("name").and_then(Value::as_str)?;
            let is_dir = item
                .get("type")
                .and_then(Value::as_str)
                .map(|t| t == "folder")
                .unwrap_or(false);
            let size = item.get("size").and_then(Value::as_u64).filter(|_| !is_dir);
            let modified_at = item
                .get("updated_at")
                .and_then(Value::as_str)
                .and_then(crate::cloudsearch::tg::parse_rfc3339_seconds);
            let mut entry = DriveEntry::new(format!("{drive_id}{ID_SEP}{file_id}"), name, is_dir);
            entry.size = size;
            entry.modified_at = modified_at;
            Some(entry)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rules;
    use std::sync::{Arc, Mutex};
    use tiny_http::Request;

    fn spawn_server<F>(handler: F) -> String
    where
        F: Fn(&Request) -> String + Send + 'static,
    {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = handler(&req);
                let resp = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        base
    }

    fn header_of(req: &Request, name: &str) -> Option<String> {
        req.headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str().to_string())
    }

    #[test]
    fn lists_personal_drive_and_resolves_download_url() {
        let seen = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let seen2 = seen.clone();
        let base = spawn_server(move |req| {
            let auth = header_of(req, "authorization").unwrap_or_default();
            seen2.lock().unwrap().push((req.url().to_string(), auth));
            if req.url().starts_with("/v2/user/get") {
                r#"{"user_id":"u1","default_drive_id":"d1"}"#.to_string()
            } else if req.url().starts_with("/adrive/v3/file/list") {
                r#"{"items":[
                    {"file_id":"f1","name":"第1集.mp4","type":"file","size":1024,"updated_at":"2026-11-17T10:00:00Z"},
                    {"file_id":"d2","name":"合集","type":"folder"}
                ]}"#
                .to_string()
            } else {
                r#"{"url":"https://dl.ali/1"}"#.to_string()
            }
        });
        let client = crate::http::client_for(&Rules::default()).unwrap();

        let entries = list(&client, &base, "tok-1", None).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "d1|f1");
        assert_eq!(entries[0].size, Some(1024));
        assert_eq!(entries[0].modified_at, Some(1_794_909_600));
        assert!(entries[1].is_dir);

        let sub = list(&client, &base, "tok-1", Some(&entries[1].id)).unwrap();
        assert_eq!(sub.len(), 2);

        assert_eq!(
            download_url(&client, &base, "tok-1", "d1|f1").unwrap(),
            "https://dl.ali/1"
        );
        let calls = seen.lock().unwrap();
        assert!(calls.iter().all(|(_, auth)| auth == "Bearer tok-1"));
        assert!(calls[0].0.starts_with("/v2/user/get"));
        assert!(calls[1].0.starts_with("/adrive/v3/file/list"));
        assert!(calls[3].0.starts_with("/v2/file/get_download_url"));
    }

    #[test]
    fn rejects_bad_entry_id_and_api_errors() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let resp = tiny_http::Response::from_string(r#"{"code":"InvalidParameter"}"#)
                    .with_status_code(400);
                let _ = req.respond(resp);
            }
        });
        let client = crate::http::client_for(&Rules::default()).unwrap();
        assert!(download_url(&client, &base, "tok", "no-sep").is_err());
        let err = download_url(&client, &base, "tok", "d1|f1").unwrap_err();
        assert!(matches!(err, CoreError::Api { code: 400, .. }));
        let err = list(&client, &base, "tok", Some("d1|d2")).unwrap_err();
        assert!(matches!(err, CoreError::Api { code: 400, .. }));
        assert!(parse_id("|f1").is_none());
        assert!(parse_id("d1|").is_none());
        assert!(parse_id("d1|f1").is_some());
    }
}
