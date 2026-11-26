//! UC 网盘协议客户端（W9.4）。
//!
//! 协议来源：`0f047745-FxzHm/deobf/com/github/catvod/parser/merge/A/F0.java`
//! （UC 引擎，`s = "http://drive.uc.cn/"`、`t = "https://pc-api.uc.cn/"`）与
//! `com/github/catvod/spider/UC.java`（282 行薄封装；分享链接正则
//! `drive.uc.cn/s/([^#/]+)(#*/list/share/([^\-]+))?`，`detailContent` 直接委托 `F0`）。
//!
//! 与 Quark 同源：端点路径一致，差异仅在查询串 `pr=UCBrowser&fr=pc`、接口根
//! `pc-api.uc.cn`、来源头 `drive.uc.cn`；请求体字段与 Quark 相同
//! （`fids` / `fid + resolutions + supports`）。
//!
//! 端点（`https://pc-api.uc.cn/1/clouddrive/`）：
//! - `POST share/sharepage/token`  `{pwd_id, passcode}` → `data.stoken`（匿名可用）
//! - `GET  share/sharepage/detail` `pwd_id/stoken/pdir_fid/_page/_size/...` → `data.list[]`
//! - `POST share/sharepage/save`   → `data.task_id`（需 Cookie）
//! - `POST file/download`          → `data[0].download_url`（需 Cookie）
//! - `POST file/v2/play`           → 转码播放信息（需 Cookie）
//!
//! 安全边界：Cookie 仅来自站点 ext（`{"cookie":"..."}` 或 `uc_cookie`），不落盘、不打日志。

use crate::error::{CoreError, Result};
use crate::model::Rules;
use crate::pan::quark::{download_body, is_video_name, parse_envelope, play_body, save_body};
use crate::pan::{cookie_required, PanEntry, PanProvider, ShareBrowse, ShareLink};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// 与原引擎一致的 UA（`F0` 的 `r(null)` 请求头）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 接口根（`F0.t`）。
pub const DRIVE_BASE: &str = "https://pc-api.uc.cn/1/clouddrive/";
/// 统一查询串（`F0` 所有请求都带）。
pub const QUERY: &str = "pr=UCBrowser&fr=pc";
/// 分享页域名（`F0.s`；仅用于文档与错误提示，token 实际走接口）。
pub const SHARE_BASE: &str = "https://drive.uc.cn/";

/// UC 文件条目（`M.b` 的 JSON 字段，与 Quark 的 `I/b` 同构）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UcFile {
    #[serde(default)]
    pub fid: String,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub pdir_fid: String,
    /// 0=目录，1=文件
    #[serde(default)]
    pub file_type: i64,
    #[serde(default)]
    pub format_type: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub thumbnail: String,
    /// 分享文件 token（转存时 `fid_token_list` 用）
    #[serde(default)]
    pub share_fid_token: String,
}

impl UcFile {
    pub fn is_dir(&self) -> bool {
        self.file_type == 0
    }

    pub fn is_video(&self) -> bool {
        !self.is_dir() && is_video_name(&self.file_name)
    }

    /// 转为统一的 [`PanEntry`]。
    pub fn to_entry(&self) -> PanEntry {
        PanEntry {
            fid: self.fid.clone(),
            name: self.file_name.clone(),
            is_dir: self.is_dir(),
            size: self.size,
            thumbnail: self.thumbnail.clone(),
        }
    }
}

/// 分享 token 响应（`data.stoken` 等）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShareToken {
    #[serde(default)]
    pub stoken: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub share_type: i64,
    #[serde(default)]
    pub expired_at: i64,
}

/// 分享目录分页结果。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShareListPage {
    #[serde(default)]
    pub list: Vec<UcFile>,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub count: i64,
}

/// UC 客户端：绑定一个 HTTP 客户端与可选 Cookie。
pub struct UcClient {
    client: reqwest::blocking::Client,
    cookie: Option<String>,
    /// 接口根（生产为 [`DRIVE_BASE`]，测试可指向 mock）。
    base: String,
}

impl UcClient {
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
            base: DRIVE_BASE.to_string(),
        }
    }

    /// 测试用：自定义接口根（末尾需带 `/`）。
    pub fn with_base(client: reqwest::blocking::Client, cookie: &str, base: &str) -> Self {
        let mut c = Self::new(client, cookie);
        c.base = base.to_string();
        c
    }

    /// 由 Rules 构建默认客户端（复用全局超时/重试配置）。
    pub fn with_rules(rules: &Rules, cookie: &str) -> Result<Self> {
        let client = crate::http::client_for(rules)?;
        Ok(Self::new(client, cookie))
    }

    /// 是否配置了 Cookie（浏览公开分享不需要；转存/直链需要）。
    pub fn has_cookie(&self) -> bool {
        self.cookie.is_some()
    }

    fn headers(&self) -> HashMap<String, String> {
        let mut h = HashMap::new();
        h.insert("User-Agent".into(), UA.into());
        h.insert("Origin".into(), "https://drive.uc.cn".into());
        h.insert("Referer".into(), "https://drive.uc.cn/".into());
        h.insert("Accept".into(), "application/json, text/plain, */*".into());
        if let Some(c) = &self.cookie {
            h.insert("Cookie".into(), c.clone());
        }
        h
    }

    /// 发请求并解析统一信封；HTTP 非 2xx 时仍尝试解析 JSON（UC 用 404 表示分享不存在）。
    fn call(&self, method: &str, url: &str, body: Option<&Value>) -> Result<Value> {
        let mut req = if method == "GET" {
            self.client.get(url)
        } else {
            self.client.post(url)
        };
        for (k, v) in self.headers() {
            req = req.header(k, v);
        }
        if let Some(b) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(serde_json::to_string(b).unwrap_or_default());
        }
        let resp = req.send().map_err(CoreError::Http)?;
        let status = resp.status().as_u16();
        let text = resp.text().map_err(CoreError::Http)?;
        parse_envelope(&text).map_err(|e| match e {
            CoreError::Json { path, msg } if status != 200 => CoreError::Api {
                code: status as i64,
                msg: format!("HTTP {status}: {msg} ({path})"),
            },
            other => other,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// 分享 token（匿名可用）。对应 `F0.G(shareId)` 的接口版（原引擎走分享页重定向取 stoken）。
    pub fn share_token(&self, share_id: &str, passcode: &str) -> Result<ShareToken> {
        let body = json!({ "pwd_id": share_id, "passcode": passcode });
        let data = self.call(
            "POST",
            &self.url(&format!("share/sharepage/token?{QUERY}")),
            Some(&body),
        )?;
        serde_json::from_value(data).map_err(|e| CoreError::Json {
            path: "data".into(),
            msg: e.to_string(),
        })
    }

    /// 分享目录一页（`pdir_fid="0"` 为根）。原引擎 `_sort=file_type:asc,file_name:asc`。
    pub fn list_page(
        &self,
        share_id: &str,
        stoken: &str,
        pdir_fid: &str,
        page: u32,
    ) -> Result<ShareListPage> {
        let stoken = utf8_percent_encode(stoken, NON_ALPHANUMERIC).to_string();
        let pdir = utf8_percent_encode(pdir_fid, NON_ALPHANUMERIC).to_string();
        let sid = utf8_percent_encode(share_id, NON_ALPHANUMERIC).to_string();
        let url = self.url(&format!(
            "share/sharepage/detail?{QUERY}&pwd_id={sid}&stoken={stoken}&pdir_fid={pdir}&force=0&_page={page}&_size=50&_fetch_banner=1&_fetch_share=1&_fetch_total=1&_sort=file_type:asc,file_name:asc"
        ));
        let data = self.call("GET", &url, None)?;
        serde_json::from_value(data).map_err(|e| CoreError::Json {
            path: "data".into(),
            msg: e.to_string(),
        })
    }

    /// 拉取目录全部页（原引擎 `_size=50` 分页、取到空页为止）。
    pub fn list_all(&self, share_id: &str, stoken: &str, pdir_fid: &str) -> Result<ShareListPage> {
        let mut out = ShareListPage::default();
        for page in 1..=20u32 {
            let p = self.list_page(share_id, stoken, pdir_fid, page)?;
            let n = p.list.len();
            out.count += p.count;
            out.total = out.total.max(p.total);
            out.list.extend(p.list);
            if n < 50 {
                break;
            }
        }
        Ok(out)
    }

    /// 浏览分享：token + 根（或子目录）列表。
    pub fn browse(&self, link: &ShareLink) -> Result<ShareBrowse> {
        let token = self.share_token(&link.share_id, &link.passcode)?;
        let pdir = if link.sub_fid.is_empty() {
            "0"
        } else {
            link.sub_fid.as_str()
        };
        let files = self
            .list_all(&link.share_id, &token.stoken, pdir)?
            .list
            .into_iter()
            .map(|f| f.to_entry())
            .collect();
        Ok(ShareBrowse {
            provider: PanProvider::Uc,
            share_id: link.share_id.clone(),
            title: token.title,
            files,
        })
    }

    /// 转存分享文件（需 Cookie）；返回 `task_id`。
    pub fn save(
        &self,
        link: &ShareLink,
        stoken: &str,
        fid: &str,
        fid_token: &str,
        to_pdir_fid: &str,
    ) -> Result<String> {
        if !self.has_cookie() {
            return Err(cookie_required(PanProvider::Uc));
        }
        let body = save_body(&link.share_id, stoken, fid, fid_token, to_pdir_fid);
        let data = self.call(
            "POST",
            &self.url(&format!("share/sharepage/save?{QUERY}")),
            Some(&body),
        )?;
        data.get("task_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| CoreError::Api {
                code: -1,
                msg: "转存响应缺少 task_id".into(),
            })
    }

    /// 取下载直链（需 Cookie）。
    pub fn download_url(&self, fid: &str) -> Result<String> {
        if !self.has_cookie() {
            return Err(cookie_required(PanProvider::Uc));
        }
        let body = download_body(fid);
        let data = self.call("POST", &self.url(&format!("file/download?{QUERY}")), Some(&body))?;
        data.as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.get("download_url"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| CoreError::Api {
                code: -1,
                msg: "下载响应缺少 download_url".into(),
            })
    }

    /// 取转码播放信息（需 Cookie）。
    pub fn play_info(&self, fid: &str) -> Result<Value> {
        if !self.has_cookie() {
            return Err(cookie_required(PanProvider::Uc));
        }
        let body = play_body(fid);
        self.call("POST", &self.url(&format!("file/v2/play?{QUERY}")), Some(&body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pan::parse_share_link;

    const TOKEN_JSON: &str = r#"{"status":200,"code":0,"message":"ok","data":{"stoken":"UCSTOKEN==","title":"UC 测试分享","share_type":0,"expired_at":4102416000000}}"#;
    const DETAIL_JSON: &str = r#"{"status":200,"code":0,"message":"ok","data":{"is_owner":0,"share":{"title":"UC 测试分享"},"list":[
        {"fid":"u1","file_name":"第1集.mp4","pdir_fid":"0","file_type":1,"format_type":"video/mp4","size":2048,"thumbnail":"https://thumb/u1.jpg","share_fid_token":"ut1"},
        {"fid":"ud1","file_name":"花絮","pdir_fid":"0","file_type":0,"format_type":"","size":0},
        {"fid":"u2","file_name":"readme.txt","pdir_fid":"0","file_type":1,"format_type":"text/plain","size":8}
    ]}}"#;
    const ERR_JSON: &str = r#"{"status":404,"code":41006,"message":"分享不存在","data":null}"#;

    #[test]
    fn error_envelope_maps_to_api_error() {
        let err = parse_envelope(ERR_JSON).unwrap_err();
        match err {
            CoreError::Api { code, msg } => {
                assert_eq!(code, 41006);
                assert_eq!(msg, "分享不存在");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[test]
    fn file_detection_and_entry_mapping() {
        let f = UcFile {
            fid: "u1".into(),
            file_name: "a.MP4".into(),
            file_type: 1,
            size: 10,
            thumbnail: "t".into(),
            ..Default::default()
        };
        assert!(f.is_video());
        assert!(!f.is_dir());
        let e = f.to_entry();
        assert_eq!(e.fid, "u1");
        assert_eq!(e.name, "a.MP4");
        assert!(!e.is_dir);
        assert_eq!(e.size, 10);
        let d = UcFile {
            file_type: 0,
            file_name: "x.mp4".into(),
            ..Default::default()
        };
        assert!(d.is_dir() && !d.is_video());
    }

    #[test]
    fn request_bodies_match_uc_engine() {
        let s = save_body("share1", "st", "fid1", "ft1", "dir0");
        assert_eq!(s["fid_list"][0], "fid1");
        assert_eq!(s["fid_token_list"][0], "ft1");
        assert_eq!(s["to_pdir_fid"], "dir0");
        assert_eq!(s["pwd_id"], "share1");
        assert_eq!(s["pdir_fid"], "0");
        assert_eq!(download_body("fid1")["fids"][0], "fid1");
        let p = play_body("fid1");
        assert_eq!(p["fid"], "fid1");
        assert_eq!(p["resolutions"], "normal,low,high,super,2k,4k");
        assert_eq!(p["supports"], "fmp4,m3u8");
    }

    #[test]
    fn transfer_requires_cookie() {
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let c = UcClient::with_base(client, "", "http://127.0.0.1:1/");
        let link = parse_share_link("https://drive.uc.cn/s/abc123").unwrap();
        assert!(matches!(
            c.save(&link, "st", "f", "t", "0"),
            Err(CoreError::Config(_))
        ));
        assert!(matches!(c.download_url("f"), Err(CoreError::Config(_))));
        assert!(matches!(c.play_info("f"), Err(CoreError::Config(_))));
    }

    #[test]
    fn browse_parses_mock_share() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let url = req.url().to_string();
                let body = if url.contains("share/sharepage/token") {
                    TOKEN_JSON
                } else if url.contains("share/sharepage/detail") {
                    // UC 引擎固定带上 pr=UCBrowser 与 file_name 排序，mock 顺带断言。
                    assert!(url.contains("pr=UCBrowser&fr=pc"), "missing UC query: {url}");
                    assert!(url.contains("_sort=file_type:asc,file_name:asc"), "bad sort: {url}");
                    DETAIL_JSON
                } else {
                    ERR_JSON
                };
                let resp = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let uc = UcClient::with_base(client, "", &base);
        let link = parse_share_link("https://drive.uc.cn/s/abcdef").unwrap();
        let b = uc.browse(&link).unwrap();
        assert_eq!(b.provider, PanProvider::Uc);
        assert_eq!(b.title, "UC 测试分享");
        assert_eq!(b.files.len(), 3);
        assert_eq!(b.files[0].name, "第1集.mp4");
        assert!(b.files[1].is_dir);
        drop(handle);
    }

    /// 真站验证：UC 接口对无效分享返回 404/41006（匿名可达、错误形态一致）。
    #[test]
    #[ignore = "live: 需要外网访问 pc-api.uc.cn"]
    fn live_error_shape() {
        let rules = Rules::default();
        let uc = UcClient::with_rules(&rules, "").unwrap();
        let err = uc.share_token("abc123", "").unwrap_err();
        match err {
            CoreError::Api { code, .. } => assert_eq!(code, 41006),
            other => panic!("expected Api, got {other:?}"),
        }
    }
}
