//! 115 个人网盘只读列表（W14 P2）。
//!
//! 端点（2026-11-17 依据公开 webapi 协议实现，未真机验证）：
//! `GET {base}files?aid=1&cid=<dir>&o=user_ptime&asc=0&offset=0&show_dir=1&limit=200&type=0`
//! （`cid=0` 为根目录），响应沿用 webapi 信封 `{state, data:[...]}`。
//!
//! 115 个人盘下载直链需要 pickcode 加密流程（RSA + `downurl` 接口），W14 P2 暂未接入，
//! 由 [`super::DriveClient::download_url`] 返回明确错误。

use serde_json::{Map, Value};

use super::DriveEntry;
use crate::error::{CoreError, Result};
use crate::pan::p115::{parse_webapi, UA};

/// 列一页目录（`parent` 为空/`0` 时为根目录）。
pub fn list(
    client: &reqwest::blocking::Client,
    base: &str,
    cookie: &str,
    parent: Option<&str>,
) -> Result<Vec<DriveEntry>> {
    let cid = parent
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("0");
    let url = format!(
        "{base}files?aid=1&cid={cid}&o=user_ptime&asc=0&offset=0&show_dir=1&limit=200&type=0"
    );
    let resp = client
        .get(&url)
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .send()
        .map_err(CoreError::Http)?;
    let status = resp.status().as_u16();
    let text = resp.text().map_err(CoreError::Http)?;
    let value = parse_webapi(&text).map_err(|e| match e {
        CoreError::Json { path, msg } if status != 200 => CoreError::Api {
            code: status as i64,
            msg: format!("HTTP {status}: {msg} ({path})"),
        },
        other => other,
    })?;
    Ok(parse_entries(&value))
}

/// 从 webapi 响应提取目录项（兼容 `data` 为数组或 `data.list` / `data.files`）。
pub fn parse_entries(value: &Value) -> Vec<DriveEntry> {
    let data = value.get("data").unwrap_or(value);
    let arr = data
        .as_array()
        .or_else(|| data.get("list").and_then(Value::as_array))
        .or_else(|| data.get("files").and_then(Value::as_array))
        .or_else(|| value.get("list").and_then(Value::as_array));
    arr.map(|items| items.iter().filter_map(entry_from_value).collect())
        .unwrap_or_default()
}

fn entry_from_value(value: &Value) -> Option<DriveEntry> {
    let obj = value.as_object()?;
    let name = get_str(obj, &["n", "file_name", "fileName", "name"]);
    if name.is_empty() {
        return None;
    }
    let fid = get_str(obj, &["fid", "file_id", "fileId"]);
    let cid = get_str(obj, &["cid"]);
    // 文件用 fid 作 id；目录用 cid。
    let id = if !fid.is_empty() { fid } else { cid.clone() };
    if id.is_empty() {
        return None;
    }
    let is_dir = obj
        .get("is_dir")
        .or_else(|| obj.get("isDir"))
        .or_else(|| obj.get("dir"))
        .and_then(Value::as_bool)
        .or_else(|| {
            obj.get("t")
                .or_else(|| obj.get("file_type"))
                .and_then(as_i64)
                .map(|t| t == 1)
        })
        .or_else(|| {
            // 目录条目通常没有 fid，只有 cid。
            let fid_empty = get_str(obj, &["fid"]).is_empty();
            (fid_empty && !cid.is_empty()).then_some(true)
        })
        .unwrap_or_else(|| !name.contains('.'));
    let size = get_i64(obj, &["s", "size", "file_size", "fileSize"])
        .filter(|s| *s >= 0)
        .map(|s| s as u64);
    let modified_at = get_i64(obj, &["te", "modified_time", "modifiedTime"])
        .filter(|t| *t > 0)
        .map(|t| if t > 1_000_000_000_000 { t / 1000 } else { t });
    Some(DriveEntry {
        id,
        name,
        is_dir,
        size,
        modified_at,
    })
}

fn get_str(obj: &Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        match obj.get(*key) {
            Some(Value::String(s)) => return s.clone(),
            Some(Value::Number(n)) => return n.to_string(),
            _ => {}
        }
    }
    String::new()
}

fn get_i64(obj: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    for key in keys {
        match obj.get(*key) {
            Some(Value::Number(n)) => {
                if let Some(v) = n.as_i64() {
                    return Some(v);
                }
                return n.as_f64().map(|f| f as i64);
            }
            Some(Value::String(s)) => {
                if let Ok(v) = s.trim().parse::<i64>() {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    None
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_personal_file_list() {
        let json: Value = serde_json::from_str(
            r#"{"state":true,"data":[
                {"fid":"f1","cid":"0","n":"第1集.mp4","s":1024,"te":1700000000},
                {"cid":"d1","n":"合集","s":0,"t":1}
            ]}"#,
        )
        .unwrap();
        let entries = parse_entries(&json);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "f1");
        assert_eq!(entries[0].name, "第1集.mp4");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].size, Some(1024));
        assert_eq!(entries[0].modified_at, Some(1_700_000_000));
        assert_eq!(entries[1].id, "d1");
        assert!(entries[1].is_dir);
    }

    #[test]
    fn parses_wrapped_list_and_skips_nameless() {
        let json: Value =
            serde_json::from_str(r#"{"data":{"list":[{"n":""},{"fid":"f2","n":"a.mkv"}]}}"#)
                .unwrap();
        let entries = parse_entries(&json);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "f2");
    }
}
