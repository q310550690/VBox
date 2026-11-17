//! 迅雷 / PikPak 共用网盘协议基座（W10.3）。
//!
//! 协议来源：
//! - `_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/parser/merge/d1.java`（迅雷 `d1` 基类）
//! - `_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/parser/merge/A/C0032l0.java`（PikPak 覆写 base）
//! - `_rea/tmp/w10-analysis.md`（端点/字段速查表）
//!
//! 两家是**同一套 drive 协议**，只换 base 与认证：
//! - 迅雷：`api-pan.xunlei.com/drive/v1/`，登录 `xluser-ssl.xunlei.com/v1/`（账号 + 可选 captcha）
//! - PikPak：`api-drive.mypikpak.com/drive/v1/`，登录 `user.mypikpak.com/v1/`（client_id/secret 静态，
//!   FxzHm 版 `O()` 对 PikPak 跳过 captcha）
//!
//! 端点（`{base}` 末尾带 `/`）：
//! - `GET  {base}share`                 `share_id/pass_code/limit/pass_code_token/thumbnail_size`
//!                                      → `pass_code_token` + `title`（匿名可用）
//! - `GET  {base}share/detail`          `share_id/parent_id/pass_code_token/order/limit/page_token`
//!                                      → `files[]` + `next_page_token`（分享目录分页）
//! - `GET  {base}share/file_info`       `share_id/pass_code/pass_code_token/file_id/space`
//!                                      → 单文件信息（含 `links`/`medias` 直链）
//! - `GET  {base}files/{fileId}`        → 自己盘内文件直链（需登录态）
//! - `POST {auth}auth/signin`           `{client_id,client_secret,username,password}` → `access_token`
//! - `POST {auth}auth/token`            `{grant_type:"refresh_token",refresh_token,...}` → 新 token
//!
//! 安全边界：token/cookie 仅来自站点 ext（`{"token":"..."}`），不落盘、不打日志；浏览公开分享
//! 不需要登录态（真站验证见 `catvod_pan_engine_live` 的 `#[ignore]` 用例）。

use crate::error::{CoreError, Result};
use crate::model::Rules;
use crate::pan::PanEntry;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::collections::HashMap;

/// 迅雷系接口通用 UA（与原引擎 `d1.java` 的 Dalvik UA 同族；PikPak 对 UA 不敏感）。
pub const UA: &str = "Dalvik/2.1.0 (Linux; U; Android 12; SM-G973N Build/PPR1.180610.011)";

/// 迅雷 drive 根（`d1.java:50`）。
pub const API_BASE: &str = "https://api-pan.xunlei.com/drive/v1/";
/// 迅雷账号根（`d1.java:92`）。
pub const AUTH_BASE: &str = "https://xluser-ssl.xunlei.com/v1/";
/// 迅雷分享 web 根（`d1.java:93`）。
pub const SHARE_WEB: &str = "https://pan.xunlei.com/s/";

/// PikPak drive 根（`C0032l0.java:15`）。
pub const PIKPAK_API_BASE: &str = "https://api-drive.mypikpak.com/drive/v1/";
/// PikPak 账号根（`C0032l0.java:24`）。
pub const PIKPAK_AUTH_BASE: &str = "https://user.mypikpak.com/v1/";
/// PikPak 分享 web 根（`C0032l0.java:25`）。
pub const PIKPAK_SHARE_WEB: &str = "https://mypikpak.com/s/";

/// PikPak client_id（`C0032l0.java:103`，静态可得）。
pub const PIKPAK_CLIENT_ID: &str = "YNxT9w7GMdWvEOKa";
/// PikPak client_secret（`C0032l0.java:108`）。
pub const PIKPAK_CLIENT_SECRET: &str = "dbw2OtmVEeuUvIptb1Coyg";
/// 迅雷 client_id（`d1.java:1136`）。
pub const XUNLEI_CLIENT_ID: &str = "Xp6vsxz_7IYVw2BB";
/// 迅雷 client_secret（`d1.java:1140`）。
pub const XUNLEI_CLIENT_SECRET: &str = "Xp6vsy4tN9toTVdMSpomVdXpRmES";

/// 分享目录缩略图尺寸（列表用小图、详情用大图，与原引擎一致）。
pub const THUMB_SMALL: &str = "SIZE_SMALL";
/// 见 [`THUMB_SMALL`]。
pub const THUMB_LARGE: &str = "SIZE_LARGE";
/// 分享目录单页上限（`d1.R` 用 200，`d1.Z` 用 100；取 100 更稳）。
pub const PAGE_LIMIT: u32 = 100;
/// 目录最多翻页数（防御异常 `next_page_token` 循环）。
pub const MAX_PAGES: u32 = 20;

/// 分享口令响应（端点 1）。
#[derive(Debug, Clone, Default)]
pub struct ShareToken {
    /// 后续 `/share/detail`、`/share/file_info` 必须带的口令。
    pub pass_code_token: String,
    /// 分享标题（部分响应在 `share.title`）。
    pub title: String,
    pub share_id: String,
}

/// 分享目录分页（端点 2）。
#[derive(Debug, Clone, Default)]
pub struct SharePage {
    pub files: Vec<DriveFile>,
    pub next_page_token: String,
}

/// drive 文件条目（`parser/merge/P/d`；Thunder/PikPak 字段同名）。
///
/// `size` 在迅雷侧是字符串、PikPak 侧是数字，故用自定义反序列化统一成 `i64`。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct DriveFile {
    #[serde(default, alias = "file_id")]
    pub id: String,
    #[serde(default, alias = "file_name")]
    pub name: String,
    /// `file` / `folder`
    #[serde(default)]
    pub kind: String,
    /// `video` / `image` / `audio` / `document` / `archive` / `other`
    #[serde(default)]
    pub category: String,
    #[serde(default, deserialize_with = "de_i64")]
    pub size: i64,
    #[serde(default, alias = "thumbnail_link")]
    pub thumbnail: String,
    /// 直链对象（`links["application/octet-stream"].{url,token}`）。
    #[serde(default)]
    pub links: Value,
    /// PikPak 直链候补（`medias[0].link.{url,token}`）。
    #[serde(default)]
    pub medias: Value,
    /// 迅雷侧 web 播放地址。
    #[serde(default)]
    pub web_content_link: String,
}

impl DriveFile {
    pub fn is_dir(&self) -> bool {
        self.kind == "folder"
    }

    /// 是否视频：优先看 `category`，退化到扩展名。
    pub fn is_video(&self) -> bool {
        !self.is_dir()
            && (self.category == "video"
                || crate::pan::p115::is_video_name(&self.name)
                || self.web_content_link.contains(".m3u8"))
    }

    /// 从文件信息里取直链（`links` 优先，其次 `medias`，最后 `web_content_link`）。
    pub fn direct_url(&self) -> Option<String> {
        extract_direct_url(&self.links)
            .or_else(|| extract_direct_url(&self.medias))
            .or_else(|| {
                let u = self.web_content_link.trim();
                if u.starts_with("http") {
                    Some(u.to_string())
                } else {
                    None
                }
            })
    }
}

/// 从 `links` / `medias` 形状里取直链。
///
/// - `{"application/octet-stream":{"url":"..."}}`（迅雷）
/// - `[{"link":{"url":"..."}}]`（PikPak `medias`）
/// - `{"url":"..."}`（扁平）
pub fn extract_direct_url(value: &Value) -> Option<String> {
    fn url_of(v: &Value) -> Option<String> {
        for key in ["url", "download_url"] {
            if let Some(u) = v.get(key).and_then(Value::as_str) {
                let u = u.trim();
                if u.starts_with("http") {
                    return Some(u.to_string());
                }
            }
        }
        None
    }
    match value {
        Value::Object(map) => {
            if let Some(u) = map
                .get("application/octet-stream")
                .and_then(|v| v.get("url"))
                .and_then(Value::as_str)
            {
                let u = u.trim();
                if u.starts_with("http") {
                    return Some(u.to_string());
                }
            }
            if let Some(u) = url_of(value) {
                return Some(u);
            }
            for v in map.values() {
                if let Some(u) = extract_direct_url(v) {
                    return Some(u);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(extract_direct_url),
        _ => None,
    }
}

/// 解析后的分享链接（provider 无关：迅雷/PikPak 同形 `.../s/{id}?pwd=xxxx`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRef {
    /// 原始链接（去掉 `#` 之后的部分）
    pub url: String,
    pub share_id: String,
    pub passcode: String,
}

/// 解析迅雷/PikPak 分享链接；不支持的形态返回 `None`。
pub fn parse_share(raw: &str) -> Option<ShareRef> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let base = raw.split('#').next().unwrap_or(raw);
    let (prefix, web) = if base.starts_with(SHARE_WEB) {
        (SHARE_WEB, "https://pan.xunlei.com/")
    } else if base.starts_with(PIKPAK_SHARE_WEB) {
        (PIKPAK_SHARE_WEB, "https://mypikpak.com/")
    } else {
        return None;
    };
    let rest = &base[prefix.len()..];
    let id: String = rest
        .chars()
        .take_while(|c| !matches!(c, '?' | '#' | '/' | '&'))
        .collect();
    if id.is_empty() {
        return None;
    }
    let passcode = base
        .split_once('?')
        .map(|(_, query)| {
            query
                .split('&')
                .find_map(|kv| {
                    let (k, v) = kv.split_once('=')?;
                    matches!(k, "pwd" | "password" | "pass_code").then(|| v.trim().to_string())
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    Some(ShareRef {
        url: format!("{web}s/{id}"),
        share_id: id,
        passcode,
    })
}

/// 需要登录态的错误（迅雷/PikPak 的 `Authorization`）。
pub fn token_required(provider: &str) -> CoreError {
    CoreError::Config(format!(
        "{provider} 需要登录态：请在站点 ext 中配置 {{\"token\":\"...\"}}"
    ))
}

/// 迅雷系 drive 客户端：一个基座同时服务迅雷与 PikPak。
pub struct XunleiDrive {
    client: reqwest::blocking::Client,
    token: Option<String>,
    /// drive 根（末尾带 `/`）
    api_base: String,
    /// 账号根（末尾带 `/`）
    auth_base: String,
    /// 展示名（错误文案用）
    provider: &'static str,
    /// 登录用 client_id
    client_id: String,
    /// 登录用 client_secret
    client_secret: String,
}

impl XunleiDrive {
    /// 迅雷默认基座。
    pub fn new(client: reqwest::blocking::Client, token: &str) -> Self {
        Self::with_bases(
            client,
            token,
            API_BASE,
            AUTH_BASE,
            "迅雷",
            XUNLEI_CLIENT_ID,
            XUNLEI_CLIENT_SECRET,
        )
    }

    /// PikPak 默认基座。
    pub fn pikpak(client: reqwest::blocking::Client, token: &str) -> Self {
        Self::with_bases(
            client,
            token,
            PIKPAK_API_BASE,
            PIKPAK_AUTH_BASE,
            "PikPak",
            PIKPAK_CLIENT_ID,
            PIKPAK_CLIENT_SECRET,
        )
    }

    /// 自定义基座（测试指向 mock；`api_base`/`auth_base` 末尾需带 `/`）。
    pub fn with_bases(
        client: reqwest::blocking::Client,
        token: &str,
        api_base: &str,
        auth_base: &str,
        provider: &'static str,
        client_id: &str,
        client_secret: &str,
    ) -> Self {
        let token = token.trim();
        Self {
            client,
            token: (!token.is_empty()).then(|| token.to_string()),
            api_base: api_base.to_string(),
            auth_base: auth_base.to_string(),
            provider,
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
        }
    }

    /// 由 Rules 构建默认迅雷客户端。
    pub fn with_rules(rules: &Rules, token: &str) -> Result<Self> {
        let client = crate::http::client_for(rules)?;
        Ok(Self::new(client, token))
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// 覆盖登录态（登录/刷新成功后调用；空串等价清空）。
    pub fn set_token(&mut self, token: &str) {
        let token = token.trim();
        self.token = (!token.is_empty()).then(|| token.to_string());
    }

    pub fn provider(&self) -> &'static str {
        self.provider
    }

    fn headers(&self) -> HashMap<String, String> {
        let mut h = HashMap::new();
        h.insert("User-Agent".into(), UA.into());
        h.insert("Accept".into(), "application/json".into());
        if let Some(t) = &self.token {
            h.insert("Authorization".into(), format!("Bearer {t}"));
        }
        h
    }

    /// 发请求并解析 JSON；`error`/`error_code` 信封映射为 `CoreError::Api`。
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
        let value: Value = serde_json::from_str(&text).map_err(|e| CoreError::Json {
            path: "body".into(),
            msg: format!("{e}; url={url}"),
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

    /// 统一取 `data`（若存在）否则整个对象。
    fn payload(value: &Value) -> &Value {
        value.get("data").filter(|v| !v.is_null()).unwrap_or(value)
    }

    fn quota(&self, key: &str) -> String {
        utf8_percent_encode(key, NON_ALPHANUMERIC).to_string()
    }

    /// 分享口令（端点 1）。匿名可调，返回 `pass_code_token` 与标题。
    pub fn share_token(&self, share_id: &str, passcode: &str) -> Result<ShareToken> {
        let url = format!(
            "{}share?share_id={}&pass_code={}&limit={PAGE_LIMIT}&pass_code_token=&thumbnail_size={THUMB_SMALL}",
            self.api_base,
            self.quota(share_id),
            self.quota(passcode),
        );
        let value = self.call("GET", &url, None)?;
        let data = Self::payload(&value);
        let title = data
            .get("title")
            .and_then(Value::as_str)
            .or_else(|| data.get("share").and_then(|s| s.get("title")).and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        let pass_code_token = data
            .get("pass_code_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if pass_code_token.is_empty() {
            return Err(CoreError::Api {
                code: -1,
                msg: format!("{} 分享口令响应缺少 pass_code_token", self.provider),
            });
        }
        Ok(ShareToken {
            pass_code_token,
            title,
            share_id: share_id.to_string(),
        })
    }

    /// 分享目录一页（端点 2）。`parent_id` 空表示根。
    pub fn share_page(
        &self,
        share_id: &str,
        pass_code_token: &str,
        parent_id: &str,
        page_token: &str,
    ) -> Result<SharePage> {
        let url = format!(
            "{}share/detail?share_id={}&parent_id={}&pass_code_token={}&order=&limit=200&page_token={}&thumbnail_size={THUMB_LARGE}",
            self.api_base,
            self.quota(share_id),
            self.quota(parent_id),
            self.quota(pass_code_token),
            self.quota(page_token),
        );
        let value = self.call("GET", &url, None)?;
        let data = Self::payload(&value);
        let files: Vec<DriveFile> = data
            .get("files")
            .cloned()
            .map(|v| serde_json::from_value::<Vec<DriveFile>>(v).unwrap_or_default())
            .unwrap_or_default();
        let next_page_token = data
            .get("next_page_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(SharePage {
            files,
            next_page_token,
        })
    }

    /// 分享目录全部条目（按 `next_page_token` 翻页）。
    pub fn share_files(
        &self,
        share_id: &str,
        passcode: &str,
        parent_id: &str,
    ) -> Result<(String, Vec<PanEntry>)> {
        let token = self.share_token(share_id, passcode)?;
        let mut out = SharePage::default();
        let mut page_token = String::new();
        for _ in 0..MAX_PAGES {
            let page = self.share_page(
                share_id,
                &token.pass_code_token,
                parent_id,
                &page_token,
            )?;
            out.files.extend(page.files);
            if page.next_page_token.is_empty() {
                break;
            }
            page_token = page.next_page_token;
        }
        Ok((token.title, out.files.into_iter().map(|f| f.into_entry()).collect()))
    }

    /// 分享内单文件信息（端点 3），含直链。
    pub fn share_file_info(
        &self,
        share_id: &str,
        passcode: &str,
        pass_code_token: &str,
        file_id: &str,
    ) -> Result<DriveFile> {
        let url = format!(
            "{}share/file_info?share_id={}&pass_code={}&pass_code_token={}&file_id={}&space=",
            self.api_base,
            self.quota(share_id),
            self.quota(passcode),
            self.quota(pass_code_token),
            self.quota(file_id),
        );
        let value = self.call("GET", &url, None)?;
        let data = Self::payload(&value);
        serde_json::from_value::<DriveFile>(data.clone()).map_err(|e| CoreError::Json {
            path: "data".into(),
            msg: e.to_string(),
        })
    }

    /// 自己盘内文件（端点 7，需登录态）。
    pub fn file(&self, file_id: &str) -> Result<DriveFile> {
        if !self.has_token() {
            return Err(token_required(self.provider));
        }
        let url = format!("{}files/{}", self.api_base, self.quota(file_id));
        let value = self.call("GET", &url, None)?;
        let data = Self::payload(&value);
        serde_json::from_value::<DriveFile>(data.clone()).map_err(|e| CoreError::Json {
            path: "data".into(),
            msg: e.to_string(),
        })
    }

    /// 分享文件直链：先 `/share/file_info`，取不到时回退 `/files/{id}`。
    pub fn download_url(
        &self,
        share_id: &str,
        passcode: &str,
        pass_code_token: &str,
        file_id: &str,
    ) -> Result<String> {
        let info = self.share_file_info(share_id, passcode, pass_code_token, file_id)?;
        if let Some(url) = info.direct_url() {
            return Ok(url);
        }
        if self.has_token() {
            let own = self.file(file_id)?;
            if let Some(url) = own.direct_url() {
                return Ok(url);
            }
        }
        Err(CoreError::Api {
            code: -1,
            msg: format!("{} 直链响应缺少 url: file_id={file_id}", self.provider),
        })
    }

    /// 账号登录（端点 8）：`POST {auth}auth/signin`。
    pub fn login(&self, username: &str, password: &str) -> Result<LoginTokens> {
        let url = format!("{}auth/signin", self.auth_base);
        let body = json!({
            "client_id": self.client_id,
            "client_secret": self.client_secret,
            "username": username,
            "password": password,
        });
        let value = self.call("POST", &url, Some(&body))?;
        parse_login(&Self::payload(&value))
    }

    /// 刷新登录态（端点 9）。
    pub fn refresh(&self, refresh_token: &str) -> Result<LoginTokens> {
        let url = format!("{}auth/token", self.auth_base);
        let body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": self.client_id,
            "client_secret": self.client_secret,
        });
        let value = self.call("POST", &url, Some(&body))?;
        parse_login(&Self::payload(&value))
    }
}

/// 登录/刷新响应。
#[derive(Debug, Clone, Default)]
pub struct LoginTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
    pub sub: String,
}

fn parse_login(data: &Value) -> Result<LoginTokens> {
    let access_token = data
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if access_token.is_empty() {
        return Err(CoreError::Api {
            code: -1,
            msg: "登录响应缺少 access_token".into(),
        });
    }
    Ok(LoginTokens {
        access_token,
        refresh_token: data
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        expires_in: data.get("expires_in").and_then(Value::as_i64).unwrap_or(0),
        token_type: data
            .get("token_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        sub: data.get("sub").and_then(Value::as_str).unwrap_or_default().to_string(),
    })
}

impl DriveFile {
    /// 转成网盘模型条目。
    pub fn into_entry(self) -> PanEntry {
        let is_dir = self.is_dir();
        PanEntry {
            fid: self.id,
            name: self.name,
            is_dir,
            size: self.size,
            thumbnail: self.thumbnail,
        }
    }
}

/// `size` 兼容字符串（迅雷）与数字（PikPak）。
fn de_i64<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::client_for;

    const SHARE_JSON: &str =
        r#"{"pass_code_token":"pct123","title":"测试分享","share_id":"abc123"}"#;
    const DETAIL_JSON: &str = r#"{"files":[
        {"id":"f1","name":"第1集.mp4","kind":"file","category":"video","size":"1024","thumbnail_link":"https://thumb/1.jpg"},
        {"id":"d1","name":"合集","kind":"folder","size":0}
    ],"next_page_token":""}"#;
    const INFO_JSON: &str = r#"{"id":"f1","name":"第1集.mp4","kind":"file","size":1024,
        "links":{"application/octet-stream":{"url":"https://cdn.test/1.mp4","token":"tk"}}}"#;
    const LOGIN_JSON: &str = r#"{"access_token":"at","refresh_token":"rt","expires_in":7200,"token_type":"Bearer","sub":"u1"}"#;

    fn serve(routes: &'static [(&'static str, &'static str)]) -> String {
        use tiny_http::{Header, Response, Server};
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", server.server_addr());
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let path = req.url().split('?').next().unwrap_or("/").to_string();
                let body = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, b)| *b)
                    .unwrap_or("{}");
                let resp = Response::from_string(body).with_header(
                    Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        addr
    }

    #[test]
    fn parse_share_rejects_unknown_hosts_and_reads_passcode() {
        let s = parse_share("https://pan.xunlei.com/s/abc123?pwd=4x9t#/list/share").unwrap();
        assert_eq!(s.share_id, "abc123");
        assert_eq!(s.passcode, "4x9t");
        assert_eq!(s.url, "https://pan.xunlei.com/s/abc123");
        let p = parse_share("https://mypikpak.com/s/xyz987").unwrap();
        assert_eq!(p.share_id, "xyz987");
        assert!(p.passcode.is_empty());
        assert_eq!(p.url, "https://mypikpak.com/s/xyz987");
        assert!(parse_share("https://example.com/s/1").is_none());
        assert!(parse_share("").is_none());
    }

    #[test]
    fn direct_url_extracts_links_and_medias() {
        let v: Value = serde_json::from_str(
            r#"{"application/octet-stream":{"url":"https://a/1.mp4","token":"t"}}"#,
        )
        .unwrap();
        assert_eq!(extract_direct_url(&v).as_deref(), Some("https://a/1.mp4"));
        let m: Value =
            serde_json::from_str(r#"[{"link":{"url":"https://b/2.m3u8"}}]"#).unwrap();
        assert_eq!(extract_direct_url(&m).as_deref(), Some("https://b/2.m3u8"));
        assert!(extract_direct_url(&serde_json::json!({"url":"ftp://x"})).is_none());
    }

    #[test]
    fn drive_file_size_accepts_string_and_number() {
        let a: DriveFile = serde_json::from_str(r#"{"id":"x","size":"2048"}"#).unwrap();
        assert_eq!(a.size, 2048);
        let b: DriveFile = serde_json::from_str(r#"{"id":"y","size":4096}"#).unwrap();
        assert_eq!(b.size, 4096);
        let c: DriveFile = serde_json::from_str(r#"{"file_id":"z","file_name":"a.MP4"}"#).unwrap();
        assert_eq!(c.id, "z");
        assert!(c.is_video());
    }

    #[test]
    fn share_token_and_detail_round_trip() {
        let base = serve(&[("/share", SHARE_JSON), ("/share/detail", DETAIL_JSON)]);
        let client = client_for(&Rules::default()).unwrap();
        let drive = XunleiDrive::with_bases(
            client,
            "",
            &format!("{base}/"),
            &format!("{base}/"),
            "测试",
            "cid",
            "csec",
        );
        let token = drive.share_token("abc123", "4x9t").unwrap();
        assert_eq!(token.pass_code_token, "pct123");
        assert_eq!(token.title, "测试分享");
        let page = drive
            .share_page("abc123", &token.pass_code_token, "", "")
            .unwrap();
        assert_eq!(page.files.len(), 2);
        assert!(page.files[0].is_video());
        assert!(page.files[1].is_dir());
        assert!(page.next_page_token.is_empty());
        let (title, files) = drive.share_files("abc123", "4x9t", "").unwrap();
        assert_eq!(title, "测试分享");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].fid, "f1");
    }

    #[test]
    fn login_and_download_url_use_expected_endpoints() {
        let base = serve(&[("/auth/signin", LOGIN_JSON), ("/share/file_info", INFO_JSON)]);
        let client = client_for(&Rules::default()).unwrap();
        let drive = XunleiDrive::with_bases(
            client,
            "",
            &format!("{base}/"),
            &format!("{base}/"),
            "测试",
            "cid",
            "csec",
        );
        let tokens = drive.login("u", "p").unwrap();
        assert_eq!(tokens.access_token, "at");
        assert_eq!(tokens.refresh_token, "rt");
        assert_eq!(tokens.expires_in, 7200);
        let url = drive.download_url("abc123", "4x9t", "pct123", "f1").unwrap();
        assert_eq!(url, "https://cdn.test/1.mp4");
    }

    #[test]
    fn file_requires_login_token() {
        let client = client_for(&Rules::default()).unwrap();
        let drive = XunleiDrive::with_bases(
            client,
            "",
            "http://127.0.0.1:1/",
            "http://127.0.0.1:1/",
            "PikPak",
            "cid",
            "csec",
        );
        assert!(!drive.has_token());
        assert!(matches!(drive.file("f1"), Err(CoreError::Config(_))));
        let mut drive = drive;
        drive.set_token("tkn");
        assert!(drive.has_token());
        assert_eq!(drive.provider(), "PikPak");
    }
}