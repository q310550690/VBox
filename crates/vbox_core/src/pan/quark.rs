//! 夸克网盘协议客户端（W9.1）。
//!
//! 协议来源：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/parser/merge/A/Y.java`
//! （1648 行，CanLit/Quark 引擎，含 UI 登录流程；此处只取其 HTTP 协议部分）与
//! `com/github/catvod/parser/merge/I/b.java`（文件模型）。
//!
//! 端点（`https://drive.quark.cn/1/clouddrive/`，统一 `pr=ucpro&fr=pc`）：
//! - `POST share/sharepage/token`      `{pwd_id, passcode}` → `data.stoken`（匿名可用）
//! - `GET  share/sharepage/detail`     `pwd_id/stoken/pdir_fid/_page/_size/...` → `data.list[]`
//! - `POST share/sharepage/save`       `{fid_list,fid_token_list,to_pdir_fid,pwd_id,stoken,pdir_fid:"0"}` → `data.task_id`（需 Cookie）
//! - `POST file/download`              `{fids:[fid]}` → `data[0].download_url`（需 Cookie）
//! - `POST file/v2/play`               `{fid,resolutions,supports}` → 转码播放信息（需 Cookie）
//!
//! 安全边界：Cookie 仅来自站点 ext（`{"cookie":"..."}`），不落盘、不打日志；浏览公开分享
//! 不需要 Cookie（真站验证见 `live_browse_public_share`）。

use crate::error::{CoreError, Result};
use crate::model::Rules;
use crate::pan::{cookie_required, PanEntry, PanProvider, ShareBrowse, ShareLink};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// 与原引擎一致的 UA（Quark 接口对 UA 不敏感，但保持同款以免风控差异）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 接口根（`S()` 里 `str` 不以 https 开头时拼接的前缀）。
pub const DRIVE_BASE: &str = "https://drive.quark.cn/1/clouddrive/";
/// 统一查询串。
pub const QUERY: &str = "pr=ucpro&fr=pc";

/// 视频扩展名（与原引擎 `E()` 的判定一致，见 Y.java 的视频过滤分支）。
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "m3u8", "avi", "ts", "flv", "wmv", "mov", "rmvb", "webm", "m4v", "mpg", "mpeg",
    "f4v", "3gp", "iso", "rm", "vob", "m2ts", "mts",
];

/// 夸克文件条目（对应 `parser/merge/I/b.java` 的 JSON 字段）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct QuarkFile {
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
    /// 分享文件的 token（转存时 `fid_token_list` 用）
    #[serde(default)]
    pub share_fid_token: String,
}

impl QuarkFile {
    pub fn is_dir(&self) -> bool {
        self.file_type == 0
    }

    pub fn is_video(&self) -> bool {
        !self.is_dir() && is_video_name(&self.file_name)
    }
}

/// 按扩展名判断是否视频（Quark 的 `format_type` 对视频给 `video/*`，但老文件可能缺省）。
pub fn is_video_name(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, ext)| VIDEO_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 分享 token 响应。
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
    pub list: Vec<QuarkFile>,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub count: i64,
}

/// 夸克客户端：绑定一个 HTTP 客户端与可选 Cookie。
pub struct QuarkClient {
    client: reqwest::blocking::Client,
    cookie: Option<String>,
    /// 接口根（生产为 [`DRIVE_BASE`]，测试可指向 mock）。
    base: String,
}

impl QuarkClient {
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

    /// 由 Rules 构建默认客户端（复用全局超时/重试配置；`Rules.headers` 不套用，避免串站）。
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
        h.insert("Origin".into(), "https://pan.quark.cn".into());
        h.insert("Referer".into(), "https://pan.quark.cn/".into());
        h.insert("Accept".into(), "application/json, text/plain, */*".into());
        if let Some(c) = &self.cookie {
            h.insert("Cookie".into(), c.clone());
        }
        h
    }

    /// 发请求并解析统一信封；HTTP 非 2xx 时仍尝试解析 JSON（Quark 用 404 表示分享不存在）。
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
            req = req.header("Content-Type", "application/json")
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

    /// 分享 token（匿名可用）。对应原引擎 `V(shareId, passcode)`。
    pub fn share_token(&self, share_id: &str, passcode: &str) -> Result<ShareToken> {
        let data = self.share_token_raw(share_id, passcode)?;
        serde_json::from_value(data).map_err(|e| CoreError::Json {
            path: "data".into(),
            msg: e.to_string(),
        })
    }

    /// 与 [`Self::share_token`] 同一请求，但返回上游原始 JSON。
    ///
    /// 仅用于「线上字段形状快照」巡检（`catvod_pan_engine_live::quark_wire_shape_matches_snapshot`）：
    /// `ShareToken` 的字段都带 `#[serde(default)]`，上游重命名时会静默变成空串，
    /// typed 解析与家族测试都不会红——形状快照是这一层唯一的保护。
    pub fn share_token_raw(&self, share_id: &str, passcode: &str) -> Result<Value> {
        let body = json!({ "pwd_id": share_id, "passcode": passcode });
        self.call(
            "POST",
            &self.url(&format!("share/sharepage/token?{QUERY}")),
            Some(&body),
        )
    }

    /// 分享目录一页（`pdir_fid="0"` 为根）。对应原引擎 `y(...sharepage/detail...)`。
    pub fn list_page(
        &self,
        share_id: &str,
        stoken: &str,
        pdir_fid: &str,
        page: u32,
    ) -> Result<ShareListPage> {
        let data = self.list_page_raw(share_id, stoken, pdir_fid, page)?;
        serde_json::from_value(data).map_err(|e| CoreError::Json {
            path: "data".into(),
            msg: e.to_string(),
        })
    }

    /// 与 [`Self::list_page`] 同一请求，但返回上游原始 JSON（字段形状巡检用，理由见
    /// [`Self::share_token_raw`]；`QuarkFile` 的字段同样全带 `#[serde(default)]`）。
    pub fn list_page_raw(
        &self,
        share_id: &str,
        stoken: &str,
        pdir_fid: &str,
        page: u32,
    ) -> Result<Value> {
        let stoken = utf8_percent_encode(stoken, NON_ALPHANUMERIC).to_string();
        let pdir = utf8_percent_encode(pdir_fid, NON_ALPHANUMERIC).to_string();
        let sid = utf8_percent_encode(share_id, NON_ALPHANUMERIC).to_string();
        let url = self.url(&format!(
            "share/sharepage/detail?{QUERY}&pwd_id={sid}&stoken={stoken}&pdir_fid={pdir}&force=0&_page={page}&_size=50&_fetch_banner=1&_fetch_share=1&_fetch_total=1&_sort=file_type:asc,updated_at:desc"
        ));
        self.call("GET", &url, None)
    }

    /// 拉取目录全部页。原引擎按 50 条分页、`i` 递增取到空页为止。
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
            .map(|f| PanEntry {
                fid: f.fid.clone(),
                name: f.file_name.clone(),
                is_dir: f.is_dir(),
                size: f.size,
                thumbnail: f.thumbnail.clone(),
            })
            .collect();
        Ok(ShareBrowse {
            provider: PanProvider::Quark,
            share_id: link.share_id.clone(),
            title: token.title,
            files,
        })
    }

    /// 转存分享文件到自己的网盘（需 Cookie）；返回 `task_id`。
    pub fn save(
        &self,
        link: &ShareLink,
        stoken: &str,
        fid: &str,
        fid_token: &str,
        to_pdir_fid: &str,
    ) -> Result<String> {
        if !self.has_cookie() {
            return Err(cookie_required(PanProvider::Quark));
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

    /// 取下载直链（需 Cookie）。原引擎 `file/download` → `data[0].download_url`。
    pub fn download_url(&self, fid: &str) -> Result<String> {
        if !self.has_cookie() {
            return Err(cookie_required(PanProvider::Quark));
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

    /// 取转码播放信息（需 Cookie）。原引擎 `file/v2/play`。
    pub fn play_info(&self, fid: &str) -> Result<Value> {
        if !self.has_cookie() {
            return Err(cookie_required(PanProvider::Quark));
        }
        let body = play_body(fid);
        self.call("POST", &self.url(&format!("file/v2/play?{QUERY}")), Some(&body))
    }
}

/// 统一信封解析：`{status, code, message, data}`，`status!=200` 或 `code!=0` 报 `CoreError::Api`。
pub fn parse_envelope(text: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(text).map_err(|e| CoreError::Json {
        path: "$".into(),
        msg: e.to_string(),
    })?;
    let status = v.get("status").and_then(|x| x.as_i64()).unwrap_or(200);
    let code = v.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
    if status != 200 || code != 0 {
        let msg = v
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("夸克接口返回错误")
            .to_string();
        return Err(CoreError::Api {
            code: if code != 0 { code } else { status },
            msg,
        });
    }
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

/// 转存请求体（原引擎 `s()` 的字段顺序不重要，字段名为准）。
pub fn save_body(
    share_id: &str,
    stoken: &str,
    fid: &str,
    fid_token: &str,
    to_pdir_fid: &str,
) -> Value {
    json!({
        "fid_list": [fid],
        "fid_token_list": [fid_token],
        "to_pdir_fid": to_pdir_fid,
        "pwd_id": share_id,
        "stoken": stoken,
        "pdir_fid": "0",
    })
}

pub fn download_body(fid: &str) -> Value {
    json!({ "fids": [fid] })
}

pub fn play_body(fid: &str) -> Value {
    json!({
        "fid": fid,
        "resolutions": "normal,low,high,super,2k,4k",
        "supports": "fmp4,m3u8",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pan::parse_share_link;

    const TOKEN_JSON: &str = r#"{"status":200,"code":0,"message":"ok","data":{"stoken":"TsyXAU2/i6qNR47l1AM3+D57Cpg3PwzZCbQk5Cl8kyo=","title":"测试分享","share_type":0,"expired_at":4102416000000}}"#;
    const DETAIL_JSON: &str = r#"{"status":200,"code":0,"message":"ok","data":{"is_owner":0,"share":{"title":"测试分享"},"list":[
        {"fid":"f1","file_name":"第1集.mp4","pdir_fid":"0","file_type":1,"format_type":"video/mp4","size":1024,"thumbnail":"https://thumb/1.jpg","share_fid_token":"tok1"},
        {"fid":"d1","file_name":"花絮","pdir_fid":"0","file_type":0,"format_type":"","size":0},
        {"fid":"f2","file_name":"readme.txt","pdir_fid":"0","file_type":1,"format_type":"text/plain","size":8}
    ]}}"#;

    #[test]
    fn envelope_ok_returns_data() {
        let d = parse_envelope(TOKEN_JSON).unwrap();
        assert_eq!(d["stoken"], "TsyXAU2/i6qNR47l1AM3+D57Cpg3PwzZCbQk5Cl8kyo=");
    }

    #[test]
    fn envelope_error_maps_to_api_error() {
        let err = parse_envelope(
            r#"{"status":404,"code":41006,"message":"分享不存在","data":null}"#,
        )
        .unwrap_err();
        match err {
            CoreError::Api { code, msg } => {
                assert_eq!(code, 41006);
                assert_eq!(msg, "分享不存在");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[test]
    fn video_and_dir_detection() {
        assert!(is_video_name("a.MP4"));
        assert!(is_video_name("第 1 集.mkv"));
        assert!(!is_video_name("a.txt"));
        assert!(!is_video_name("noext"));
        let f = QuarkFile {
            file_type: 1,
            file_name: "x.m3u8".into(),
            ..Default::default()
        };
        assert!(f.is_video() && !f.is_dir());
        let d = QuarkFile {
            file_type: 0,
            file_name: "x.mp4".into(),
            ..Default::default()
        };
        assert!(d.is_dir() && !d.is_video());
    }

    #[test]
    fn request_bodies_match_engine_fields() {
        let s = save_body("share1", "st", "fid1", "ft1", "dir0");
        assert_eq!(s["fid_list"][0], "fid1");
        assert_eq!(s["fid_token_list"][0], "ft1");
        assert_eq!(s["to_pdir_fid"], "dir0");
        assert_eq!(s["pwd_id"], "share1");
        assert_eq!(s["stoken"], "st");
        assert_eq!(s["pdir_fid"], "0");
        assert_eq!(download_body("fid1")["fids"][0], "fid1");
        let p = play_body("fid1");
        assert_eq!(p["fid"], "fid1");
        assert_eq!(p["supports"], "fmp4,m3u8");
    }

    #[test]
    fn save_requires_cookie() {
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let c = QuarkClient::with_base(client, "", "http://127.0.0.1:1/");
        assert!(matches!(
            c.save(
                &parse_share_link("https://pan.quark.cn/s/abc").unwrap(),
                "st",
                "f",
                "t",
                "0"
            ),
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
                    DETAIL_JSON
                } else {
                    r#"{"status":404,"code":1,"message":"not found"}"#
                };
                let resp = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        let client = crate::http::client_for(&Rules::default()).unwrap();
        let q = QuarkClient::with_base(client, "", &base);
        let link = parse_share_link("https://pan.quark.cn/s/24f565ff8b77").unwrap();
        let b = q.browse(&link).unwrap();
        assert_eq!(b.title, "测试分享");
        assert_eq!(b.files.len(), 3);
        assert_eq!(b.files[0].name, "第1集.mp4");
        assert!(!b.files[0].is_dir);
        assert!(b.files[1].is_dir);
        // 故意不 join：mock server 线程随测试进程回收。
        drop(handle);
    }

    /// 真站验证：公开分享 `24f565ff8b77`（源码示例链接）可匿名浏览。
    #[test]
    #[ignore = "live: 需要外网访问 drive.quark.cn"]
    fn live_browse_public_share() {
        let rules = Rules::default();
        let q = QuarkClient::with_rules(&rules, "").unwrap();
        let link = parse_share_link("https://pan.quark.cn/s/24f565ff8b77").unwrap();
        let b = q.browse(&link).unwrap();
        assert!(!b.title.is_empty(), "分享标题为空");
        assert!(!b.files.is_empty(), "分享文件列表为空");
    }
}
