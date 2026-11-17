//! 夸克 / UC 个人网盘只读列表与下载直链（W14 P2）。
//!
//! 两个 provider 同源：端点路径一致，仅接口根与查询串不同（复用 `pan::{quark,uc}` 常量）。
//! 端点（2026-11-17 依据公开协议实现，未真机验证）：
//! - 列表：`GET {base}file/sort?{query}&pdir_fid=<dir>&_page=1&_size=200&_fetch_total=1&_sort=file_type:asc,updated_at:desc`
//!   （`pdir_fid=0` 为根目录）→ 信封 `data.list[]`
//! - 直链：`POST {base}file/download?{query}` body `{"fids":[fid]}` → `data[0].download_url`

use serde_json::Value;

use super::DriveEntry;
use crate::error::{CoreError, Result};
use crate::pan::quark::{download_body, parse_envelope, UA};

/// 列一页目录（`parent` 为空/`0` 时为根目录）。
pub fn list(
    client: &reqwest::blocking::Client,
    base: &str,
    query: &str,
    provider: &str,
    cookie: &str,
    parent: Option<&str>,
) -> Result<Vec<DriveEntry>> {
    let pdir = parent
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("0");
    let url = format!(
        "{base}file/sort?{query}&pdir_fid={pdir}&_page=1&_size=200&_fetch_total=1&_sort=file_type:asc,updated_at:desc"
    );
    let resp = client
        .get(&url)
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .send()
        .map_err(CoreError::Http)?;
    let status = resp.status().as_u16();
    let text = resp.text().map_err(CoreError::Http)?;
    let data = parse_envelope(&text).map_err(|e| with_status(e, status, provider))?;
    Ok(parse_entries(&data))
}

/// 取个人盘文件下载直链。
pub fn download_url(
    client: &reqwest::blocking::Client,
    base: &str,
    query: &str,
    provider: &str,
    cookie: &str,
    fid: &str,
) -> Result<String> {
    let url = format!("{base}file/download?{query}");
    let body = download_body(fid);
    let resp = client
        .post(&url)
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .json(&body)
        .send()
        .map_err(CoreError::Http)?;
    let status = resp.status().as_u16();
    let text = resp.text().map_err(CoreError::Http)?;
    let data = parse_envelope(&text).map_err(|e| with_status(e, status, provider))?;
    data.as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("download_url"))
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CoreError::Api {
            code: -1,
            msg: format!("{provider} 下载响应缺少 download_url"),
        })
}

/// 从信封 `data` 提取目录项（`file/sort` 的 `data.list[]`）。
pub fn parse_entries(data: &Value) -> Vec<DriveEntry> {
    data.get("list")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(entry_from_value).collect())
        .unwrap_or_default()
}

fn entry_from_value(value: &Value) -> Option<DriveEntry> {
    let obj = value.as_object()?;
    let id = ["fid", "file_id", "fileId"]
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::to_string)?;
    let name = ["file_name", "fileName", "name"]
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::to_string)?;
    if id.is_empty() || name.is_empty() {
        return None;
    }
    // `dir` 为布尔（真=目录）；`file_type` 仅作为兜底（0=目录、1=文件）。
    let is_dir = obj
        .get("dir")
        .and_then(Value::as_bool)
        .or_else(|| obj.get("file_type").and_then(Value::as_i64).map(|t| t == 0))
        .unwrap_or_else(|| !name.contains('.'));
    let size = obj
        .get("size")
        .and_then(Value::as_i64)
        .filter(|s| *s >= 0)
        .map(|s| s as u64);
    let modified_at = obj.get("updated_at").and_then(Value::as_i64).map(|t| {
        if t > 1_000_000_000_000 {
            t / 1000
        } else {
            t
        }
    });
    Some(DriveEntry {
        id,
        name,
        is_dir,
        size,
        modified_at,
    })
}

/// 非 2xx 且响应体不是合法信封时，补上 HTTP 状态与 provider，避免只报 JSON 解析失败。
fn with_status(err: CoreError, status: u16, provider: &str) -> CoreError {
    match err {
        CoreError::Json { path, msg } if status != 200 => CoreError::Api {
            code: status as i64,
            msg: format!("{provider} HTTP {status}: {msg} ({path})"),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quark_personal_list() {
        let data: Value = serde_json::from_str(
            r#"{"list":[
                {"fid":"f1","file_name":"第1集.mp4","dir":false,"size":2048,"updated_at":1700000000},
                {"fid":"d1","file_name":"合集","dir":true,"size":0,"updated_at":1700000100}
            ]}"#,
        )
        .unwrap();
        let entries = parse_entries(&data);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "f1");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].size, Some(2048));
        assert!(entries[1].is_dir);
        assert_eq!(entries[1].modified_at, Some(1_700_000_100));
    }

    #[test]
    fn skips_incomplete_entries() {
        let data: Value =
            serde_json::from_str(r#"{"list":[{"fid":"f1"},{"file_name":"x"}]}"#).unwrap();
        assert!(parse_entries(&data).is_empty());
    }
}
