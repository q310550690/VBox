//! 迅雷个人网盘只读列表与下载直链（W14 P2）。
//!
//! 端点（2026-11-17 依据公开协议与 `pan/xunlei.rs` 既有常量实现，未真机验证）：
//! - 列表：`GET {base}files?parent_id=<dir>&limit=100`（根目录省略 `parent_id`），
//!   头 `Authorization: Bearer <access_token>`；响应 `{files:[...],next_page_token}`。
//! - 直链：`GET {base}files/<id>` → 文件对象里的 `links` / `medias` / `web_content_link`。
//!
//! PikPak 与迅雷同构（`https://api-drive.mypikpak.com/drive/v1/`），由
//! [`crate::drive::pikpak`] 复用本模块。

use serde_json::Value;

use super::DriveEntry;
use crate::error::{CoreError, Result};
use crate::pan::xunlei::UA;

/// 列一页目录（`parent` 为空时为根目录）。
pub fn list(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
    parent: Option<&str>,
) -> Result<Vec<DriveEntry>> {
    let parent_id = parent.map(str::trim).filter(|s| !s.is_empty());
    let url = match parent_id {
        Some(id) => format!("{base}files?parent_id={id}&limit=100"),
        None => format!("{base}files?limit=100"),
    };
    let value = get_json(client, token, &url)?;
    Ok(parse_entries(&value))
}

/// 取个人盘文件直链。
pub fn download_url(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
    file_id: &str,
) -> Result<String> {
    let url = format!("{base}files/{file_id}");
    let value = get_json(client, token, &url)?;
    let file = value.get("file").unwrap_or(&value);
    direct_url(file).ok_or_else(|| CoreError::Api {
        code: -1,
        msg: format!("迅雷直链响应缺少 url: file_id={file_id}"),
    })
}

fn get_json(client: &reqwest::blocking::Client, token: &str, url: &str) -> Result<Value> {
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("User-Agent", UA)
        .send()
        .map_err(CoreError::Http)?;
    let status = resp.status().as_u16();
    let text = resp.text().map_err(CoreError::Http)?;
    let value: Value = serde_json::from_str(&text).map_err(|e| CoreError::Json {
        path: "body".into(),
        msg: format!("{e}; HTTP {status}"),
    })?;
    if let Some(err) = value.get("error").and_then(Value::as_str) {
        let msg = value
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or(err);
        return Err(CoreError::Api {
            code: value
                .get("error_code")
                .and_then(Value::as_i64)
                .unwrap_or(status as i64),
            msg: msg.to_string(),
        });
    }
    Ok(value)
}

/// 从列表响应提取目录项（`files[]`，字段兼容 `id`/`file_id`、`name`/`file_name`）。
pub fn parse_entries(value: &Value) -> Vec<DriveEntry> {
    value
        .get("files")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(entry_from_value).collect())
        .unwrap_or_default()
}

fn entry_from_value(value: &Value) -> Option<DriveEntry> {
    let obj = value.as_object()?;
    let id = ["id", "file_id", "fileId"]
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::to_string)?;
    let name = ["name", "file_name", "fileName"]
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::to_string)?;
    if id.is_empty() || name.is_empty() {
        return None;
    }
    let is_dir = obj
        .get("kind")
        .and_then(Value::as_str)
        .map(|kind| kind == "folder")
        .or_else(|| obj.get("is_dir").and_then(Value::as_bool))
        .unwrap_or_else(|| !name.contains('.'));
    let size = obj
        .get("size")
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|s| *s >= 0)
        .map(|s| s as u64);
    let modified_at = obj
        .get("modified_time")
        .or_else(|| obj.get("modifiedTime"))
        .and_then(modified_at_of);
    Some(DriveEntry {
        id,
        name,
        is_dir,
        size,
        modified_at,
    })
}

/// `modified_time` 兼容 Unix 秒/毫秒数字与 RFC3339 字符串。
fn modified_at_of(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .map(|t| if t > 1_000_000_000_000 { t / 1000 } else { t }),
        Value::String(s) => {
            let s = s.trim();
            if let Ok(t) = s.parse::<i64>() {
                return Some(if t > 1_000_000_000_000 { t / 1000 } else { t });
            }
            crate::cloudsearch::tg::parse_rfc3339_seconds(s)
        }
        _ => None,
    }
}

/// 从文件对象里取直链（`links` 优先，其次 `medias[].link`，最后 `web_content_link`）。
fn direct_url(file: &Value) -> Option<String> {
    let from_links = file
        .get("links")
        .and_then(Value::as_object)
        .and_then(|links| {
            links
                .get("application/octet-stream")
                .or_else(|| links.values().next())
        })
        .and_then(|link| link.get("url"))
        .and_then(Value::as_str);
    if let Some(url) = from_links.filter(|url| !url.is_empty()) {
        return Some(url.to_string());
    }
    let from_medias = file
        .get("medias")
        .and_then(Value::as_array)
        .and_then(|medias| medias.first())
        .and_then(|media| media.get("link"))
        .and_then(|link| link.get("url"))
        .and_then(Value::as_str);
    if let Some(url) = from_medias.filter(|url| !url.is_empty()) {
        return Some(url.to_string());
    }
    file.get("web_content_link")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xunlei_file_list() {
        let value: Value = serde_json::from_str(
            r#"{"files":[
                {"id":"f1","name":"第1集.mp4","kind":"file","size":"2048","modified_time":"2026-11-17T10:00:00Z"},
                {"id":"d1","name":"合集","kind":"folder","size":0},
                {"id":"f2","name":"无大小","kind":"file"}
            ],"next_page_token":"next"}"#,
        )
        .unwrap();
        let entries = parse_entries(&value);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "f1");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].size, Some(2048));
        assert_eq!(entries[0].modified_at, Some(1_794_909_600));
        assert!(entries[1].is_dir);
        assert_eq!(entries[2].size, None);
    }

    #[test]
    fn picks_direct_url_from_file_object() {
        let value: Value = serde_json::from_str(
            r#"{"file":{"id":"f1","links":{"application/octet-stream":{"url":"https://dl/1"}}}}"#,
        )
        .unwrap();
        assert_eq!(direct_url(&value["file"]).as_deref(), Some("https://dl/1"));

        let value: Value =
            serde_json::from_str(r#"{"medias":[{"link":{"url":"https://dl/2"}}]}"#).unwrap();
        assert_eq!(direct_url(&value).as_deref(), Some("https://dl/2"));

        let value: Value = serde_json::from_str(r#"{"web_content_link":"https://dl/3"}"#).unwrap();
        assert_eq!(direct_url(&value).as_deref(), Some("https://dl/3"));
    }
}
