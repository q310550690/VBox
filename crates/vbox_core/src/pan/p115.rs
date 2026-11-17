//! 115 网盘协议客户端（W9.4 追加，第五个 provider）。
//!
//! 协议来源：
//! - `_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/P115.java`（237 行，分享适配器：
//!   正则 `115.com/s/([^\?#/]+)`，`password`/`cid` 查询参数）；
//! - `parser/merge/A/C0030k0.java`（1164 行，115 引擎：本地代理 `127.0.0.1:10078` +
//!   WebView 拦截 `115.com/.../share/list` 的 XHR 响应（含 `"files"` 字段），并从
//!   `Set-Cookie` 保存 `UID/SEID/CID`；下载走 `115/download?pick_code=...`）。
//!
//! 桌面端无 WebView，改为直接调用 webapi 端点（与原页面 XHR 同一套 JSON）：
//! - `GET  https://webapi.115.com/share/snap?share_code=..&offset=..&limit=..&receive_code=..`
//!   → `{state, data:{count, list:[...]}}`（匿名可用；`state=false` 时 `error/errno` 给出原因）
//! - `GET  https://webapi.115.com/share/download?share_code=..&receive_code=..&file_id=..`
//!   → `{state, data:{url}}`（需要 Cookie；另外原引擎会带 `Range` 与 `downloadinfo.Header`）
//! - `POST https://webapi.115.com/share/receive`（转存，需要 Cookie；本轮未实现）
//!
//! 安全边界：Cookie 只来自站点 ext（`{"p115_cookie":"..."}`），不落盘、不打日志。

use crate::error::{CoreError, Result};
use crate::model::Rules;
use crate::pan::{cookie_required, PanEntry, PanProvider, ShareBrowse, ShareLink};
use serde_json::Value;
use std::collections::HashMap;

/// 与 115 网页端一致的 UA。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// webapi 根（原页面 XHR 的根）。
pub const WEB_BASE: &str = "https://webapi.115.com/";
/// 分享页根（仅用于生成可读链接，不参与请求）。
pub const SHARE_BASE: &str = "https://115.com/s/";
/// 分页大小（snap 的 `limit`，115 网页端缺省 20，此处放宽到 100）。
pub const PAGE_SIZE: i64 = 100;
/// 最大分页数（防御异常响应导致死循环）。
pub const MAX_PAGES: u32 = 20;

/// 视频扩展名（与原引擎 `E()` 的判定一致）。
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "m3u8", "avi", "ts", "flv", "wmv", "mov", "rmvb", "webm", "m4v", "mpg", "mpeg",
    "f4v", "3gp", "iso", "rm", "vob", "m2ts", "mts",
];

/// 115 文件条目。
///
/// 字段名兼容两套命名：115 网页 XHR 的缩写（`n/s/pc`）与 webapi 的驼峰（`fileName/fileSize`）。
/// `is_dir` 优先取显式布尔字段，其次按 `t == 1`（网页端目录类型），最后按名称是否含扩展名兜底。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct P115File {
    pub cid: String,
    pub name: String,
    pub size: i64,
    pub is_dir: bool,
    pub parent: String,
    pub pick_code: String,
    pub thumbnail: String,
}

impl P115File {
    pub fn is_video(&self) -> bool {
        !self.is_dir && is_video_name(&self.name)
    }

    pub fn to_entry(&self) -> PanEntry {
        PanEntry {
            fid: self.cid.clone(),
            name: self.name.clone(),
            is_dir: self.is_dir,
            size: self.size,
            thumbnail: self.thumbnail.clone(),
        }
    }
}

/// 从 JSON 值解析条目，容忍字段别名与字符串化数字。
pub fn file_from_value(v: &Value) -> Option<P115File> {
    let obj = v.as_object()?;
    let get_str = |keys: &[&str]| -> String {
        for k in keys {
            if let Some(x) = obj.get(*k) {
                match x {
                    Value::String(s) => return s.clone(),
                    Value::Number(n) => return n.to_string(),
                    _ => {}
                }
            }
        }
        String::new()
    };
    let get_i64 = |keys: &[&str]| -> i64 {
        for k in keys {
            if let Some(x) = obj.get(*k) {
                match x {
                    Value::Number(n) => return n.as_i64().unwrap_or(0),
                    Value::String(s) => return s.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        0
    };
    let cid = get_str(&["cid", "file_id", "fid", "fileId"]);
    let name = get_str(&["n", "file_name", "fileName", "name"]);
    if cid.is_empty() && name.is_empty() {
        return None;
    }
    let size = get_i64(&["s", "file_size", "fileSize", "size"]);
    let parent = get_str(&["pc", "parent_id", "parentId", "pid"]);
    let pick_code = get_str(&["pick_code", "pickCode", "pc_code"]);
    let thumbnail = get_str(&["ico", "thumb", "thumbnail"]);
    let is_dir = obj
        .get("is_dir")
        .or_else(|| obj.get("isDir"))
        .or_else(|| obj.get("dir"))
        .and_then(Value::as_bool)
        .or_else(|| {
            obj.get("t")
                .or_else(|| obj.get("file_type"))
                .or_else(|| obj.get("fileType"))
                .and_then(|x| match x {
                    Value::Number(n) => n.as_i64(),
                    Value::String(s) => s.trim().parse().ok(),
                    _ => None,
                })
                .map(|t| t == 1)
        })
        .unwrap_or_else(|| !name.contains('.'));
    Some(P115File {
        cid,
        name,
        size,
        is_dir,
        parent,
        pick_code,
        thumbnail,
    })
}

/// 按扩展名判断是否视频。
pub fn is_video_name(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, ext)| VIDEO_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 分享快照一页。
#[derive(Debug, Clone, Default)]
pub struct SnapPage {
    pub count: i64,
    pub files: Vec<P115File>,
}

/// 115 客户端：绑定一个 HTTP 客户端与可选 Cookie。
pub struct P115Client {
    client: reqwest::blocking::Client,
    cookie: Option<String>,
    base: String,
}

impl P115Client {
    /// `cookie` 为空串视为匿名（只能浏览公开分享）。
    pub fn new(client: reqwest::blocking::Client, cookie: &str) -> Self {
        let cookie = cookie.trim();
        Self {
            client,
            cookie: if cookie.is_empty() {
                None
            } else {
                Some(cookie.to_string())
            },
            base: WEB_BASE.to_string(),
        }
    }

    /// 测试用：自定义接口根（末尾需带 `/`）。
    pub fn with_base(client: reqwest::blocking::Client, cookie: &str, base: &str) -> Self {
        let mut c = Self::new(client, cookie);
        c.base = base.to_string();
        c
    }

    /// 由 Rules 构建默认客户端（复用全局超时/重试配置；`Rules.headers` 不套用，避免串站）。
    pub fn with_rules(rules: &Rules, cookie: &str) -> Result<Self> {
        let client = crate::http::client_for(rules)?;
        Ok(Self::new(client, cookie))
    }

    /// 是否配置了 Cookie（浏览公开分享不需要；下载/转存需要）。
    pub fn has_cookie(&self) -> bool {
        self.cookie.is_some()
    }

    fn headers(&self) -> HashMap<String, String> {
        let mut h = HashMap::new();
        h.insert("User-Agent".into(), UA.into());
        h.insert("Referer".into(), "https://115.com/".into());
        h.insert("Origin".into(), "https://115.com".into());
        h.insert("Accept".into(), "application/json, text/plain, */*".into());
        if let Some(c) = &self.cookie {
            h.insert("Cookie".into(), c.clone());
        }
        h
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// GET JSON；115 的 webapi 用 `state` 表达成败（HTTP 仍可能是 200）。
    fn get_json(&self, url: &str) -> Result<Value> {
        let mut req = self.client.get(url);
        for (k, v) in self.headers() {
            req = req.header(k, v);
        }
        let resp = req.send().map_err(CoreError::Http)?;
        let status = resp.status().as_u16();
        let text = resp.text().map_err(CoreError::Http)?;
        parse_webapi(&text).map_err(|e| match e {
            CoreError::Json { path, msg } if status != 200 => CoreError::Api {
                code: status as i64,
                msg: format!("HTTP {status}: {msg} ({path})"),
            },
            other => other,
        })
    }

    /// 分享快照一页（匿名可用）。`offset` 从 0 开始。
    pub fn snap(&self, share_code: &str, receive_code: &str, offset: i64) -> Result<SnapPage> {
        let url = self.url(&format!(
            "share/snap?share_code={}&offset={offset}&limit={PAGE_SIZE}&receive_code={}",
            encode(share_code),
            encode(receive_code),
        ));
        let value = self.get_json(&url)?;
        Ok(snap_from_value(&value))
    }

    /// 拉取分享全部文件（快照是扁平列表，按 `offset` 翻页直到不足一页）。
    pub fn list_all(&self, share_code: &str, receive_code: &str) -> Result<SnapPage> {
        let mut out = SnapPage::default();
        for page in 0..MAX_PAGES {
            let offset = (page as i64) * PAGE_SIZE;
            let p = self.snap(share_code, receive_code, offset)?;
            let got = p.files.len() as i64;
            out.count = out.count.max(p.count);
            out.files.extend(p.files);
            if got < PAGE_SIZE {
                break;
            }
        }
        Ok(out)
    }

    /// 浏览分享（扁平列表按 `parent` 过滤出当前目录项；根目录为 `""`/`0`）。
    pub fn browse(&self, link: &ShareLink) -> Result<ShareBrowse> {
        let all = self.list_all(&link.share_id, &link.passcode)?;
        let sub = link.sub_fid.trim();
        let files = all
            .files
            .into_iter()
            .filter(|f| {
                if sub.is_empty() {
                    let p = f.parent.trim();
                    p.is_empty() || p == "0" || p == "-1" || p == "root"
                } else {
                    f.parent.trim() == sub
                }
            })
            .map(|f| f.to_entry())
            .collect();
        Ok(ShareBrowse {
            provider: PanProvider::P115,
            share_id: link.share_id.clone(),
            title: String::new(),
            files,
        })
    }

    /// 取下载直链（需 Cookie）。原引擎走 `115/download?pick_code=...`，webapi 侧用 file_id。
    pub fn download_url(&self, share_code: &str, receive_code: &str, file_id: &str) -> Result<String> {
        if !self.has_cookie() {
            return Err(cookie_required(PanProvider::P115));
        }
        let url = self.url(&format!(
            "share/download?share_code={}&receive_code={}&file_id={}",
            encode(share_code),
            encode(receive_code),
            encode(file_id),
        ));
        let value = self.get_json(&url)?;
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        let mut url = data
            .get("url")
            .or_else(|| data.get("download_url"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            let msg = value
                .get("error")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("下载直链响应缺少 url");
            return Err(CoreError::Api { code: -1, msg: msg.to_string() });
        }
        if url.starts_with("//") {
            url = format!("https:{url}");
        }
        Ok(url)
    }
}

/// 解析 115 webapi 响应：`state=false` 视为 API 错误（errno 作为 code）。
pub fn parse_webapi(text: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|e| CoreError::Json {
        path: "body".into(),
        msg: e.to_string(),
    })?;
    let state = value.get("state").and_then(Value::as_bool).unwrap_or(true);
    if !state {
        let code = value.get("errno").and_then(Value::as_i64).unwrap_or(-1);
        let msg = value
            .get("error")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("115 接口返回 state=false");
        return Err(CoreError::Api { code, msg: msg.to_string() });
    }
    Ok(value)
}

/// 从快照响应中提取 `count` 与文件列表（兼容 `data.list` / `data.files` / 顶层 `list`）。
pub fn snap_from_value(value: &Value) -> SnapPage {
    let data = value.get("data").unwrap_or(value);
    let count = data.get("count").and_then(Value::as_i64).unwrap_or(0);
    let arr = data
        .get("list")
        .or_else(|| data.get("files"))
        .or_else(|| value.get("list"))
        .and_then(Value::as_array);
    let files = arr
        .map(|a| a.iter().filter_map(file_from_value).collect())
        .unwrap_or_default();
    SnapPage { count, files }
}

fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pan::parse_share_link;

    const SNAP_JSON: &str = r#"{"state":true,"data":{"count":3,"list":[
        {"cid":"f1","n":"第1集.mp4","s":"1048576","pc":"0","pick_code":"pc1","ico":"https://i/1.jpg"},
        {"cid":"d1","n":"花絮","s":0,"pc":"0","t":1},
        {"cid":"f2","n":"readme.txt","s":8,"pc":"d1"}
    ]}}"#;

    #[test]
    fn webapi_ok_returns_value() {
        let v = parse_webapi(SNAP_JSON).unwrap();
        assert_eq!(v["data"]["count"], 3);
    }

    #[test]
    fn webapi_false_maps_to_api_error() {
        let err = parse_webapi(r#"{"state":false,"error":"参数错误。","errno":990002}"#)
            .unwrap_err();
        match err {
            CoreError::Api { code, msg } => {
                assert_eq!(code, 990002);
                assert!(msg.contains("参数错误"));
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[test]
    fn file_from_value_aliases_and_string_numbers() {
        let v: Value = serde_json::from_str(
            r#"{"file_id":"a1","file_name":"x.mkv","file_size":"123","parent_id":"9","is_dir":false}"#,
        )
        .unwrap();
        let f = file_from_value(&v).unwrap();
        assert_eq!(f.cid, "a1");
        assert_eq!(f.size, 123);
        assert_eq!(f.parent, "9");
        assert!(!f.is_dir && f.is_video());

        let d: Value = serde_json::from_str(r#"{"cid":"d","n":"目录","t":1}"#).unwrap();
        let f = file_from_value(&d).unwrap();
        assert!(f.is_dir && !f.is_video());
        // 无显式类型时，按名称是否含扩展名兜底。
        let d2: Value = serde_json::from_str(r#"{"cid":"d2","n":"无扩展名"}"#).unwrap();
        assert!(file_from_value(&d2).unwrap().is_dir);
    }

    #[test]
    fn snap_from_value_variants() {
        let p = snap_from_value(&serde_json::from_str::<Value>(SNAP_JSON).unwrap());
        assert_eq!(p.count, 3);
        assert_eq!(p.files.len(), 3);
        assert_eq!(p.files[0].name, "第1集.mp4");
        // 兼容 data.files 与顶层 list。
        let alt: Value = serde_json::from_str(r#"{"data":{"files":[{"cid":"z","n":"a.mp4"}]}}"#).unwrap();
        assert_eq!(snap_from_value(&alt).files.len(), 1);
        let top: Value = serde_json::from_str(r#"{"list":[{"cid":"y","n":"b.m3u8"}]}"#).unwrap();
        assert_eq!(snap_from_value(&top).files.len(), 1);
    }

    #[test]
    fn video_ext_detection() {
        assert!(is_video_name("a.MP4"));
        assert!(is_video_name("合集.mkv"));
        assert!(!is_video_name("a.txt"));
        assert!(!is_video_name("noext"));
    }

    #[test]
    fn download_requires_cookie() {
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let c = P115Client::with_base(client, "", "http://127.0.0.1:1/");
        assert!(matches!(
            c.download_url("sw123", "", "f1"),
            Err(CoreError::Config(_)) | Err(CoreError::Api { .. })
        ));
    }

    #[test]
    fn browse_filters_by_parent_via_mock() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let url = req.url().to_string();
                let body = if url.contains("share/snap") {
                    SNAP_JSON
                } else if url.contains("share/download") {
                    r#"{"state":true,"data":{"url":"https://cdn.test/a.m3u8"}}"#
                } else {
                    r#"{"state":false,"error":"not found","errno":404}"#
                };
                let resp = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let c = P115Client::with_base(client, "UID=1; SEID=2", &base);
        let link = parse_share_link("https://115.com/s/sw123abc?password=xy9z").unwrap();
        let b = c.browse(&link).unwrap();
        assert_eq!(b.provider, PanProvider::P115);
        assert_eq!(b.files.len(), 2);
        assert_eq!(b.files[0].name, "第1集.mp4");
        assert!(b.files[1].is_dir);
        assert_eq!(c.download_url("sw123abc", "xy9z", "f1").unwrap(), "https://cdn.test/a.m3u8");
        drop(handle);
    }

    /// 真站验证：无效分享码返回 `state=false`/`errno 990002`（端点形态权威）。
    #[test]
    #[ignore = "live: 需要外网访问 webapi.115.com"]
    fn live_error_shape() {
        let rules = Rules::default();
        let c = P115Client::with_rules(&rules, "").unwrap();
        let err = c.snap("invalid", "", 0).unwrap_err();
        match err {
            CoreError::Api { code, .. } => assert_eq!(code, 990002),
            other => panic!("expected Api, got {other:?}"),
        }
    }
}
