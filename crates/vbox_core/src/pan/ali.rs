//! 阿里云盘（Aliyun Drive / Alipan）分享协议客户端（W9.3）。
//!
//! 协议来源：`0f047745-FxzHm/deobf/com/github/catvod/parser/merge/A/E.java`
//! （2105 行，AliYun 引擎；含扫码登录与转存）与 `com/github/catvod/spider/Ali.java`
//! （221 行，薄封装：分享链接正则 → 引擎 → `vod_play_url`）。
//!
//! 端点（默认 `https://api.aliyundrive.com/`）：
//! - `POST adrive/v3/share_link/get_share_by_anonymous` `{share_id}` → 分享元信息（匿名）
//! - `POST v2/share_link/get_share_token` `{share_id, share_pwd}` → `share_token`
//! - `POST adrive/v3/file/list`（头 `x-share-token`）`{share_id,parent_file_id,limit,order_by,…}` → `items[]`
//! - `POST v2/file/get_share_link_download_url`（头 `x-share-token`）`{share_id,file_id}` → `url`
//! - `POST v2/file/get_share_link_video_preview_play_info`（头 `x-share-token`）→ `live_transcoding_task_list[]`
//!
//! 安全边界：Cookie 仅来自站点 ext（`{"cookie":"…"}` 或 `{"ali_cookie":"…"}`），不落盘、不打日志。

use crate::error::{CoreError, Result};
use crate::model::Rules;
use crate::pan::{cookie_required, PanEntry, PanProvider, ShareBrowse, ShareLink};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// 与原引擎一致的 UA（`E.I()` 的 Chrome/122）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 默认接口根（`E.u()` 在路径不以 https 开头时拼接的前缀）。
pub const API_BASE: &str = "https://api.aliyundrive.com/";
/// 分享页 Referer / Origin。
pub const REFERER: &str = "https://www.alipan.com/";
/// Web 端分享通道标识（`E.K()`）。
pub const X_CANARY: &str = "client=web,app=share,version=v2.3.1";
/// 单页大小。
pub const PAGE_SIZE: u32 = 100;
/// 目录分页上限（防止异常 marker 死循环）。
pub const MAX_PAGES: usize = 20;

/// 视频扩展名（与原引擎 `E()` 的判定一致）。
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "m3u8", "avi", "ts", "flv", "wmv", "mov", "rmvb", "webm", "m4v", "mpg", "mpeg",
    "f4v", "3gp", "iso", "rm", "vob", "m2ts", "mts",
];

/// Ali 文件条目（`adrive/v3/file/list` 的 `items[]`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AliFile {
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub file_name: String,
    /// `file` / `folder`
    #[serde(default, rename = "type")]
    pub file_type: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub thumbnail: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub updated_at: String,
}

impl AliFile {
    pub fn is_dir(&self) -> bool {
        self.file_type == "folder"
    }

    pub fn is_video(&self) -> bool {
        !self.is_dir() && is_video_name(&self.file_name)
    }
}

/// 文件名是否视频扩展名。
pub fn is_video_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    match lower.rsplit_once('.') {
        Some((_, ext)) => VIDEO_EXTS.contains(&ext),
        None => false,
    }
}

/// 分享元信息（`get_share_by_anonymous`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShareInfo {
    #[serde(default)]
    pub share_name: String,
    #[serde(default)]
    pub share_id: String,
    #[serde(default)]
    pub file_infos: Vec<AliFile>,
}

/// 分享 token（`get_share_token`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShareToken {
    #[serde(default)]
    pub share_token: String,
    #[serde(default)]
    pub expire_time: i64,
}

impl ShareToken {
    pub fn is_valid(&self) -> bool {
        !self.share_token.is_empty()
    }
}

/// 目录一页。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShareListPage {
    #[serde(default)]
    pub items: Vec<AliFile>,
    #[serde(default)]
    pub next_marker: String,
}

/// 阿里云盘客户端：绑定 HTTP 客户端、可选 Cookie 与接口根。
pub struct AliClient {
    client: reqwest::blocking::Client,
    cookie: Option<String>,
    base: String,
}

impl AliClient {
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
            base: API_BASE.to_string(),
        }
    }

    /// 测试用：自定义接口根（末尾需带 `/`）。
    pub fn with_base(client: reqwest::blocking::Client, cookie: &str, base: &str) -> Self {
        let mut c = Self::new(client, cookie);
        c.base = base.to_string();
        c
    }

    /// 由 Rules 构建默认客户端。
    pub fn with_rules(rules: &Rules, cookie: &str) -> Result<Self> {
        let client = crate::http::client_for(rules)?;
        Ok(Self::new(client, cookie))
    }

    /// 是否配置了 Cookie（浏览公开分享不需要；转存需要）。
    pub fn has_cookie(&self) -> bool {
        self.cookie.is_some()
    }

    fn headers(&self, share_token: Option<&str>) -> HashMap<String, String> {
        let mut h = HashMap::new();
        h.insert("User-Agent".into(), UA.into());
        h.insert("Referer".into(), REFERER.into());
        h.insert("Origin".into(), "https://www.alipan.com".into());
        h.insert("Accept".into(), "application/json, text/plain, */*".into());
        h.insert("X-Canary".into(), X_CANARY.into());
        if let Some(t) = share_token {
            h.insert("x-share-token".into(), t.to_string());
        }
        if let Some(c) = &self.cookie {
            h.insert("Cookie".into(), c.clone());
        }
        h
    }

    /// POST JSON 并解析；Ali 错误为 `{code,message}` 且带 4xx 状态码。
    fn call(&self, path: &str, body: &Value, share_token: Option<&str>) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let mut req = self.client.post(&url).header("Content-Type", "application/json");
        for (k, v) in self.headers(share_token) {
            req = req.header(k, v);
        }
        let resp = req
            .body(serde_json::to_string(body).unwrap_or_default())
            .send()
            .map_err(CoreError::Http)?;
        let status = resp.status().as_u16();
        let text = resp.text().map_err(CoreError::Http)?;
        let value: Value = serde_json::from_str(&text).map_err(|e| CoreError::Json {
            path: path.to_string(),
            msg: e.to_string(),
        })?;
        if status >= 400 {
            let code = value.get("code").and_then(|v| v.as_str()).unwrap_or("");
            let msg = value.get("message").and_then(|v| v.as_str()).unwrap_or("");
            return Err(CoreError::Api {
                code: status as i64,
                msg: format!("{code}: {msg}"),
            });
        }
        Ok(value)
    }

    /// 分享元信息（匿名可用）。对应原引擎 `W(shareUrl)` → `get_share_by_anonymous`。
    pub fn share_info(&self, share_id: &str) -> Result<ShareInfo> {
        let v = self.call(
            "adrive/v3/share_link/get_share_by_anonymous",
            &json!({ "share_id": share_id }),
            None,
        )?;
        serde_json::from_value(v).map_err(|e| CoreError::Json {
            path: "share_info".into(),
            msg: e.to_string(),
        })
    }

    /// 分享 token（匿名可用；带提取码的分享需要 passcode）。对应原引擎 `X(shareId, pwd)`。
    pub fn share_token(&self, share_id: &str, passcode: &str) -> Result<ShareToken> {
        let v = self.call(
            "v2/share_link/get_share_token",
            &json!({ "share_id": share_id, "share_pwd": passcode }),
            None,
        )?;
        serde_json::from_value(v).map_err(|e| CoreError::Json {
            path: "share_token".into(),
            msg: e.to_string(),
        })
    }

    /// 目录一页（`parent_file_id="root"` 为根）；分页用返回的 `next_marker`。
    pub fn list_page(
        &self,
        share_id: &str,
        share_token: &str,
        parent: &str,
        marker: &str,
    ) -> Result<ShareListPage> {
        let mut body = json!({
            "share_id": share_id,
            "parent_file_id": parent,
            "limit": PAGE_SIZE,
            "order_by": "name",
            "order_direction": "ASC",
        });
        if !marker.is_empty() {
            body["marker"] = json!(marker);
        }
        let v = self.call("adrive/v3/file/list", &body, Some(share_token))?;
        serde_json::from_value(v).map_err(|e| CoreError::Json {
            path: "file/list".into(),
            msg: e.to_string(),
        })
    }

    /// 递归/分页列出全部条目（上限 [`MAX_PAGES`] 页）。
    pub fn list_all(
        &self,
        share_id: &str,
        share_token: &str,
        parent: &str,
    ) -> Result<ShareListPage> {
        let mut out = ShareListPage::default();
        let mut marker = String::new();
        for _ in 0..MAX_PAGES {
            let page = self.list_page(share_id, share_token, parent, &marker)?;
            let n = page.items.len();
            out.items.extend(page.items);
            marker = page.next_marker;
            if n == 0 || marker.is_empty() {
                break;
            }
        }
        Ok(out)
    }

    /// 浏览分享：token + 根（或子目录）列表。
    pub fn browse(&self, link: &ShareLink) -> Result<ShareBrowse> {
        let token = self.share_token(&link.share_id, &link.passcode)?;
        if !token.is_valid() {
            return Err(CoreError::Config(format!(
                "Ali 分享 {} 需要提取码或已失效（share_token 为空）",
                link.share_id
            )));
        }
        if !link.passcode.is_empty() && token.share_token.is_empty() {
            return Err(CoreError::Config(format!(
                "Ali 分享 {} 提取码错误",
                link.share_id
            )));
        }
        let title = self
            .share_info(&link.share_id)
            .map(|i| i.share_name)
            .unwrap_or_default();
        let parent = if link.sub_fid.is_empty() {
            "root".to_string()
        } else {
            link.sub_fid.clone()
        };
        let page = self.list_all(&link.share_id, &token.share_token, &parent)?;
        let files = page
            .items
            .into_iter()
            .map(|f| PanEntry {
                fid: f.file_id.clone(),
                name: f.file_name.clone(),
                is_dir: f.is_dir(),
                size: f.size,
                thumbnail: f.thumbnail.clone(),
            })
            .collect();
        Ok(ShareBrowse {
            provider: PanProvider::Ali,
            share_id: link.share_id.clone(),
            title,
            files,
        })
    }

    /// 分享文件下载直链（`get_share_link_download_url`）。
    ///
    /// 匿名请求可用；若服务端要求登录（401/403）且未配置 Cookie，返回明确的配置错误。
    pub fn download_url(
        &self,
        share_id: &str,
        file_id: &str,
        share_token: &str,
    ) -> Result<String> {
        let v = match self.call(
            "v2/file/get_share_link_download_url",
            &json!({ "share_id": share_id, "file_id": file_id }),
            Some(share_token),
        ) {
            Ok(v) => v,
            Err(CoreError::Api { code, .. })
                if (code == 401 || code == 403) && !self.has_cookie() =>
            {
                return Err(cookie_required(PanProvider::Ali));
            }
            Err(e) => return Err(e),
        };
        v.get("url")
            .and_then(|u| u.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| CoreError::Api {
                code: 0,
                msg: "Ali 未返回下载直链".into(),
            })
    }

    /// 视频转码播放地址（`get_share_link_video_preview_play_info`，优先高码率）。
    pub fn play_url(&self, share_id: &str, file_id: &str, share_token: &str) -> Result<String> {
        let v = self.call(
            "v2/file/get_share_link_video_preview_play_info",
            &json!({
                "file_id": file_id,
                "share_id": share_id,
                "template_id": "",
                "category": "live_transcoding",
                "url_expire_sec": 600,
                "get_preview_url": true,
                "get_subtitle_info": true,
                "drive_id": ""
            }),
            Some(share_token),
        )?;
        pick_play_url(&v).ok_or_else(|| CoreError::Api {
            code: 0,
            msg: "Ali 未返回转码播放地址".into(),
        })
    }
}

/// 转码模板优先级（原引擎 `T()`：UHD > QHD > FHD > HD > SD > LD，最后回退第一项）。
pub const TEMPLATE_ORDER: &[&str] = &["UHD", "QHD", "FHD", "HD", "SD", "LD"];

/// 从播放信息响应中挑选地址。
pub fn pick_play_url(v: &Value) -> Option<String> {
    let list = v
        .get("video_preview_play_info")
        .and_then(|i| i.get("live_transcoding_task_list"))
        .and_then(|l| l.as_array())?;
    let mut urls: Vec<(&str, &str)> = Vec::new();
    for item in list {
        let template = item.get("template_id").and_then(|t| t.as_str()).unwrap_or("");
        let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
        if !url.is_empty() {
            urls.push((template, url));
        }
    }
    for want in TEMPLATE_ORDER {
        if let Some((_, url)) = urls.iter().find(|(t, _)| t == want) {
            return Some((*url).to_string());
        }
    }
    urls.first().map(|(_, u)| (*u).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pan::parse_share_link;

    const ANON_JSON: &str = r#"{"share_name":"测试分享","share_id":"abc123","file_infos":[{"file_id":"root","file_name":"测试分享","type":"folder"}]}"#;
    const TOKEN_JSON: &str = r#"{"share_token":"tok123","expire_time":4102416000}"#;
    const LIST1_JSON: &str = r#"{"items":[
        {"file_id":"dir1","file_name":"第1季","type":"folder","size":0},
        {"file_id":"f1","file_name":"S01E01.mp4","type":"file","size":1024,"thumbnail":"https://thumb/1.jpg"}
    ],"next_marker":"m2"}"#;
    const LIST2_JSON: &str = r#"{"items":[
        {"file_id":"f2","file_name":"readme.txt","type":"file","size":8}
    ],"next_marker":""}"#;
    const DOWNLOAD_JSON: &str = r#"{"url":"https://cdn.test/ali/file.mp4","expiration":"2026-11-11T20:00:00Z"}"#;
    const PLAY_JSON: &str = r#"{"video_preview_play_info":{"live_transcoding_task_list":[
        {"template_id":"LD","url":"https://cdn.test/ld.m3u8"},
        {"template_id":"FHD","url":"https://cdn.test/fhd.m3u8"},
        {"template_id":"UHD","url":"https://cdn.test/uhd.m3u8"}
    ]}}"#;
    const EMPTY_TOKEN_JSON: &str = r#"{"share_token":"","expire_time":0}"#;

    #[test]
    fn video_and_dir_detection() {
        assert!(is_video_name("a.MP4"));
        assert!(is_video_name("第 1 集.mkv"));
        assert!(!is_video_name("a.txt"));
        assert!(!is_video_name("noext"));
        let f = AliFile {
            file_type: "file".into(),
            file_name: "x.m3u8".into(),
            ..Default::default()
        };
        assert!(f.is_video() && !f.is_dir());
        let d = AliFile {
            file_type: "folder".into(),
            file_name: "x.mp4".into(),
            ..Default::default()
        };
        assert!(d.is_dir() && !d.is_video());
    }

    #[test]
    fn play_url_prefers_uhd() {
        let v: Value = serde_json::from_str(PLAY_JSON).unwrap();
        assert_eq!(pick_play_url(&v).as_deref(), Some("https://cdn.test/uhd.m3u8"));
        let v2: Value = serde_json::from_str(
            r#"{"video_preview_play_info":{"live_transcoding_task_list":[{"template_id":"HD","url":"https://cdn.test/hd.m3u8"},{"template_id":"SD","url":"https://cdn.test/sd.m3u8"}]}}"#,
        )
        .unwrap();
        assert_eq!(pick_play_url(&v2).as_deref(), Some("https://cdn.test/hd.m3u8"));
        assert_eq!(pick_play_url(&json!({})), None);
    }

    fn start_mock() -> (String, std::thread::JoinHandle<()>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let url = req.url().to_string();
                let has_marker = {
                    let mut buf = String::new();
                    let _ = std::io::Read::read_to_string(req.as_reader(), &mut buf);
                    buf.contains("\"marker\":\"m2\"")
                };
                let body = if url.contains("get_share_by_anonymous") {
                    ANON_JSON
                } else if url.contains("get_share_token") {
                    TOKEN_JSON
                } else if url.contains("file/list") {
                    if has_marker {
                        LIST2_JSON
                    } else {
                        LIST1_JSON
                    }
                } else if url.contains("get_share_link_download_url") {
                    DOWNLOAD_JSON
                } else if url.contains("video_preview_play_info") {
                    PLAY_JSON
                } else {
                    r#"{"code":"NotFound","message":"no route"}"#
                };
                let status = if url.contains("no_route") { 404 } else { 200 };
                let resp = tiny_http::Response::from_string(body)
                    .with_status_code(status)
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    );
                let _ = req.respond(resp);
            }
        });
        (base, handle)
    }

    #[test]
    fn browse_parses_mock_share() {
        let (base, handle) = start_mock();
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let c = AliClient::with_base(client, "", &base);
        let link = parse_share_link("https://www.alipan.com/s/abc123").unwrap();
        let b = c.browse(&link).unwrap();
        assert_eq!(b.provider, PanProvider::Ali);
        assert_eq!(b.title, "测试分享");
        assert_eq!(b.files.len(), 3);
        assert_eq!(b.files[0].fid, "dir1");
        assert!(b.files[0].is_dir);
        assert_eq!(b.files[1].name, "S01E01.mp4");
        assert_eq!(b.files[1].size, 1024);
        drop(handle);
    }

    #[test]
    fn download_and_play_urls() {
        let (base, handle) = start_mock();
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let c = AliClient::with_base(client, "", &base);
        assert_eq!(
            c.download_url("abc", "f1", "tok123").unwrap(),
            "https://cdn.test/ali/file.mp4"
        );
        assert_eq!(
            c.play_url("abc", "f1", "tok123").unwrap(),
            "https://cdn.test/uhd.m3u8"
        );
        drop(handle);
    }

    #[test]
    fn empty_share_token_is_config_error() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let resp = tiny_http::Response::from_string(EMPTY_TOKEN_JSON).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let c = AliClient::with_base(client, "", &base);
        let link = parse_share_link("https://www.alipan.com/s/abc123").unwrap();
        assert!(matches!(c.browse(&link), Err(CoreError::Config(_))));
        drop(handle);
    }

    /// 真站验证：错误分享 id 返回 404 `NotFound.ShareLink`（证明端点与协议形态）。
    #[test]
    #[ignore = "live: 需要外网访问 api.aliyundrive.com"]
    fn live_error_shape() {
        let rules = Rules::default();
        let c = AliClient::with_rules(&rules, "").unwrap();
        match c.share_info("abc123") {
            Err(CoreError::Api { code, msg }) => {
                assert_eq!(code, 404);
                assert!(msg.contains("NotFound"), "msg={msg}");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[test]
    fn http_error_maps_to_api_error() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let resp = tiny_http::Response::from_string(
                    r#"{"code":"NotFound.ShareLink","message":"The resource share_link cannot be found. Please check."}"#,
                )
                .with_status_code(404)
                .with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let c = AliClient::with_base(client, "", &base);
        match c.share_info("bad") {
            Err(CoreError::Api { code, msg }) => {
                assert_eq!(code, 404);
                assert!(msg.contains("NotFound.ShareLink"), "msg={msg}");
            }
            other => panic!("expected Api, got {other:?}"),
        }
        drop(handle);
    }
}
