//! 在线字幕源（当前已接入：SubHD）
//!
//! 设计：
//! - 每个站点一个适配函数（`search_subhd` / `resolve_subhd_download`），对外统一成
//!   [`OnlineSubtitle`] 与「解析出直链」两步，前端只认 `source` + `id`；
//! - 站点域名经常被墙/换域名：先探测一组候选域名，选第一个可用的并缓存在进程内；
//! - SubHD 的下载是需要会话的 3 步流程（GET 详情页拿 cookie → `prepare-download` →
//!   `down` 换直链），所以这里用**带 cookie 库存的阻塞客户端**，逐次请求；
//! - 所有请求都走本模块自己的客户端：UA 伪装、20s 超时、放行自签名证书（影视站常见）。
//!
//! 说明：2026-11 实测国内其它常见字幕站（字幕库 zimuku、伪射手 assrt、人人影视 yysub、
//! 电影字幕网 ddzimu、TLF 等）要么域名停放、要么整站验证码、要么连接被重置，暂不可用；
//! 这里保留统一的源枚举，后续按同样形状补 `search_xxx` 即可。

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use scraper::{ElementRef, Html, Selector};
use serde::Serialize;

use crate::error::{CoreError, Result};

/// 浏览器 UA（SubHD 对默认 rust 客户端会拒绝）
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// SubHD 域名候选（按可用性从高到低；页面底部自述 subhd.me / subhd.one 为主域名）
const SUBHD_DOMAINS: [&str; 6] = [
    "https://subhd.me",
    "https://subhd.tv",
    "https://subhd.cc",
    "https://subhd.com",
    "https://subhd.one",
    "https://subhd.top",
];

/// 在线字幕源描述（给前端的下拉/标签用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSourceInfo {
    /// 源 id（调用搜索/下载时回传）
    pub id: String,
    /// 展示名
    pub name: String,
    /// 一句话说明
    pub hint: String,
}

/// 当前可用的在线字幕源
pub fn sources() -> Vec<SubtitleSourceInfo> {
    vec![SubtitleSourceInfo {
        id: "subhd".to_string(),
        name: "SubHD".to_string(),
        hint: "中文字幕站（简/繁/双语，ASS/SRT/SUP）".to_string(),
    }]
}

/// 一条在线字幕搜索结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineSubtitle {
    /// 站内 id（SubHD 为 `/a/{id}` 中的 id）
    pub id: String,
    /// 源 id（"subhd"）
    pub source: String,
    /// 片名 / 剧名
    pub title: String,
    /// 字幕版本名（通常包含片源版本、语言、特效说明）
    pub name: String,
    /// 字幕格式：ASS / SRT / SUP / SSA / VTT …
    pub format: String,
    /// 语言标签：双语 / 简体 / 繁体 / 英语 …
    pub languages: Vec<String>,
    /// 来源徽标：官方字幕 / 其他来源 / 精选推荐
    pub origin: String,
    /// 字幕组（没有则为空）
    pub group: String,
    /// 文件大小（站点原样，如 "180k"）
    pub size: String,
    /// 下载次数（站点原样）
    pub downloads: String,
    /// 发布时间（站点原样，如 "08-05 07:54"）
    pub date: String,
    /// 发布人
    pub uploader: String,
    /// 详情页地址
    pub page_url: String,
}

/* ---------------- 对外入口 ---------------- */

/// 按关键词搜索在线字幕（`page` 从 1 开始）。
pub fn search(source: &str, query: &str, page: u32) -> Result<Vec<OnlineSubtitle>> {
    search_inner(source, query, page).map_err(|e| crate::sources::map_aux_error("subtitle", e))
}

fn search_inner(source: &str, query: &str, page: u32) -> Result<Vec<OnlineSubtitle>> {
    match source.trim() {
        "" | "subhd" => search_subhd(query, page),
        other => Err(CoreError::Config(format!("未知的在线字幕源: {other}"))),
    }
}

/// 把一条搜索结果解析成可下载的直链 URL（SubHD 需要先建立会话）。
pub fn resolve_download(source: &str, id: &str) -> Result<String> {
    resolve_download_inner(source, id).map_err(|e| crate::sources::map_aux_error("subtitle", e))
}

fn resolve_download_inner(source: &str, id: &str) -> Result<String> {
    match source.trim() {
        "" | "subhd" => resolve_subhd_download(id),
        other => Err(CoreError::Config(format!("未知的在线字幕源: {other}"))),
    }
}

/// 下载直链到本地文件（目录不存在会自动创建）。
pub fn download_to(url: &str, dest: &Path) -> Result<()> {
    download_to_inner(url, dest).map_err(|e| crate::sources::map_aux_error("subtitle", e))
}

fn download_to_inner(url: &str, dest: &Path) -> Result<()> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(CoreError::Config(format!("非法的字幕下载地址: {url}")));
    }
    let client = client()?;
    let mut resp = client.get(url).send()?;
    if !resp.status().is_success() {
        return Err(CoreError::Config(format!(
            "字幕下载失败: HTTP {}（{url}）",
            resp.status()
        )));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(dest)?;
    std::io::copy(&mut resp, &mut file)?;
    Ok(())
}

/* ---------------- HTTP ---------------- */

/// 字幕站专用客户端：伪装 UA + cookie 库存 + 放行自签名证书。
fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(UA)
        .cookie_store(true)
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(CoreError::Http)
}

/// 进程内缓存 SubHD 的可用域名（避免每次搜索都把候选域名试一遍）。
fn subhd_base(client: &reqwest::blocking::Client) -> Result<String> {
    static BASE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    let cache = BASE.get_or_init(|| Mutex::new(None));
    if let Some(base) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .cloned()
    {
        return Ok(base);
    }

    let mut last_error = String::new();
    for domain in SUBHD_DOMAINS {
        // 用首页探测：只要 200 就认为该域名可用（部分镜像首页有反爬但搜索正常，
        // 这里不做内容校验，真实失败会在随后的搜索请求里暴露）。
        match client.get(domain).send() {
            Ok(resp) if resp.status().is_success() => {
                *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(domain.to_string());
                return Ok(domain.to_string());
            }
            Ok(resp) => last_error = format!("{} 返回 HTTP {}", domain, resp.status()),
            Err(err) => last_error = format!("{domain} 连接失败: {err}"),
        }
    }
    Err(CoreError::Config(format!(
        "SubHD 所有候选域名均不可用（{last_error}）"
    )))
}

/// 丢弃缓存的 SubHD 域名（测试 / 域名切换后重试用）。
pub fn reset_subhd_base() {
    static BASE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    let cache = BASE.get_or_init(|| Mutex::new(None));
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// 路径段百分号编码（中文片名）。
fn encode_segment(text: &str) -> String {
    percent_encoding::utf8_percent_encode(text, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/* ---------------- SubHD：搜索 ---------------- */

fn search_subhd(query: &str, page: u32) -> Result<Vec<OnlineSubtitle>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(CoreError::Config("搜索关键词为空".to_string()));
    }
    let client = client()?;
    let base = subhd_base(&client)?;
    let page = page.max(1);
    let path = if page <= 1 {
        format!("/search/{}", encode_segment(query))
    } else {
        format!("/search/{}/{}", encode_segment(query), page)
    };
    let url = format!("{base}{path}");
    let resp = client
        .get(&url)
        .header(reqwest::header::REFERER, &base)
        .send()?;
    if !resp.status().is_success() {
        return Err(CoreError::Config(format!(
            "SubHD 搜索失败: HTTP {}（{url}）",
            resp.status()
        )));
    }
    let html = resp.text()?;
    Ok(parse_subhd_search(&html, &base))
}

/// 解析 SubHD 搜索结果页（`/search/{q}`）。
///
/// 结果卡片：`div.bg-white.shadow-sm.rounded-3.mb-4`；每张卡片里片名与字幕版本名
/// 各有一个 `a[href^="/a/"]`，语言是 `span.p-1.fw-bold`，格式是 `span.p-1.text-secondary`。
pub fn parse_subhd_search(html: &str, base: &str) -> Vec<OnlineSubtitle> {
    let doc = Html::parse_document(html);
    let card_sel = match Selector::parse("div.bg-white.shadow-sm.rounded-3.mb-4") {
        Ok(sel) => sel,
        Err(_) => return Vec::new(),
    };
    let title_sel = Selector::parse("a.link-dark.align-middle").ok();
    let name_sel = Selector::parse("div.view-text a").ok();
    let lang_sel = Selector::parse("div.text-truncate span.p-1.fw-bold").ok();
    let format_sel = Selector::parse("div.text-truncate span.p-1.text-secondary").ok();
    let origin_sel = Selector::parse("div.text-truncate span.rounded.p-1.me-1.text-white").ok();
    let group_sel = Selector::parse("div.float-end a[href^='/zu/']").ok();
    let uploader_sel = Selector::parse("div.pt-1.f12 a[href^='/u/']").ok();
    let meta_sel = Selector::parse("div.pt-2.f12 span.align-text-top.me-3").ok();

    let mut items = Vec::new();
    for card in doc.select(&card_sel) {
        let Some(id) = first_link_id(&card) else {
            continue;
        };
        let title = title_sel
            .as_ref()
            .and_then(|sel| {
                card.select(sel)
                    .next()
                    .map(|el| clean_text(&el.text().collect::<String>()))
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| id.clone());
        let name = name_sel
            .as_ref()
            .and_then(|sel| {
                card.select(sel)
                    .next()
                    .map(|el| clean_text(&el.text().collect::<String>()))
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| title.clone());

        let languages: Vec<String> = match &lang_sel {
            Some(sel) => card
                .select(sel)
                .map(|el| clean_text(&el.text().collect::<String>()))
                .filter(|text| !text.is_empty())
                .collect(),
            None => Vec::new(),
        };
        let format = format_sel
            .as_ref()
            .and_then(|sel| {
                card.select(sel)
                    .last()
                    .map(|el| clean_text(&el.text().collect::<String>()))
            })
            .unwrap_or_default();
        let origin = origin_sel
            .as_ref()
            .and_then(|sel| {
                card.select(sel)
                    .next()
                    .map(|el| clean_text(&el.text().collect::<String>()))
            })
            .unwrap_or_default();
        let group = group_sel
            .as_ref()
            .and_then(|sel| {
                card.select(sel)
                    .next()
                    .map(|el| clean_text(&el.text().collect::<String>()))
            })
            .unwrap_or_default();
        let uploader = uploader_sel
            .as_ref()
            .and_then(|sel| {
                card.select(sel)
                    .next()
                    .map(|el| clean_text(&el.text().collect::<String>()))
            })
            .unwrap_or_default();

        // 元信息三个 span：文件大小 / 下载次数 / 发布时间
        let meta: Vec<String> = match &meta_sel {
            Some(sel) => card
                .select(sel)
                .map(|el| clean_text(&el.text().collect::<String>()))
                .collect(),
            None => Vec::new(),
        };
        items.push(OnlineSubtitle {
            page_url: format!("{base}/a/{id}"),
            id,
            source: "subhd".to_string(),
            title,
            name,
            format,
            languages,
            origin,
            group,
            size: meta.first().cloned().unwrap_or_default(),
            downloads: meta.get(1).cloned().unwrap_or_default(),
            date: meta.get(2).cloned().unwrap_or_default(),
            uploader,
        });
    }
    items
}

/// 取卡片里第一个 `/a/{id}` 链接的 id。
fn first_link_id(card: &ElementRef<'_>) -> Option<String> {
    let selector = Selector::parse("a[href^='/a/']").ok()?;
    let href = card.select(&selector).next()?.value().attr("href")?;
    let id = href.trim_start_matches("/a/").trim_matches('/');
    // 只接受站内字母数字 id，避免脏 href 被当成文件名/URL 用
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(id.to_string())
}

/* ---------------- SubHD：下载直链 ---------------- */

fn resolve_subhd_download(id: &str) -> Result<String> {
    let id = id.trim();
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(CoreError::Config(format!("非法的字幕 id: {id}")));
    }
    let client = client()?;
    let base = subhd_base(&client)?;
    let page_url = format!("{base}/a/{id}");

    // 1) 打开详情页：站点会下发 tk_* cookie
    let resp = client
        .get(&page_url)
        .header(reqwest::header::REFERER, &base)
        .send()?;
    if !resp.status().is_success() {
        return Err(CoreError::Config(format!(
            "打开字幕详情页失败: HTTP {}（{page_url}）",
            resp.status()
        )));
    }
    let _ = resp.text();

    // 2) 申请下载：站点下发 down_* cookie（作用域 /api/sub/down）
    let prepare_url = format!("{base}/api/sub/prepare-download");
    let prepare = client
        .post(&prepare_url)
        .header(reqwest::header::REFERER, &page_url)
        .header(reqwest::header::ORIGIN, &base)
        .json(&serde_json::json!({ "sid": id }))
        .send()?;
    let prepare_json: serde_json::Value = prepare.json()?;
    if prepare_json.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return Err(CoreError::Config(format!(
            "字幕准备下载失败: {}",
            json_message(&prepare_json)
        )));
    }

    // 3) 打开临时下载页（`/down/{id}`）：这一步会建立「临时页面」会话，
    //    跳过它直接调 `/api/sub/down` 会得到「时间过长本临时页面已经失效」。
    let down_page = format!("{base}/down/{id}");
    let down_page_resp = client
        .get(&down_page)
        .header(reqwest::header::REFERER, &page_url)
        .send()?;
    if !down_page_resp.status().is_success() {
        return Err(CoreError::Config(format!(
            "打开字幕下载页失败: HTTP {}（{down_page}）",
            down_page_resp.status()
        )));
    }
    let _ = down_page_resp.text();

    // 4) 换取真实文件地址（单文件直链，可能是 .ass/.srt/.sup/.zip/.rar）
    let down_url = format!("{base}/api/sub/down");
    let down = client
        .post(&down_url)
        .header(reqwest::header::REFERER, &down_page)
        .header(reqwest::header::ORIGIN, &base)
        .json(&serde_json::json!({ "sid": id }))
        .send()?;
    let down_json: serde_json::Value = down.json()?;
    if down_json.get("success").and_then(|v| v.as_bool()) != Some(true)
        || down_json.get("pass").and_then(|v| v.as_bool()) != Some(true)
    {
        return Err(CoreError::Config(format!(
            "字幕下载校验失败: {}",
            json_message(&down_json)
        )));
    }
    let url = down_json
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(CoreError::Config(format!("字幕直链无效: {url}")));
    }
    Ok(url)
}

/// 从站点 JSON 里取可读错误文案（`msg` / `message` / `error`）。
fn json_message(value: &serde_json::Value) -> String {
    for key in ["msg", "message", "error"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
    }
    "未知错误".to_string()
}

/// 合并空白 + 去首尾空格（站点 HTML 里缩进很多）。
fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 搜索结果解析：片名/版本名/语言/格式/大小/下载量/发布人 全部就位。
    #[test]
    fn parses_subhd_search_fixture() {
        let html = include_str!("../tests/fixtures/subhd_search.html");
        let items = parse_subhd_search(html, "https://subhd.me");
        assert_eq!(items.len(), 2, "fixture 里应有 2 条结果");
        let first = &items[0];
        assert_eq!(first.id, "gb6JIH");
        assert_eq!(first.source, "subhd");
        assert_eq!(first.title, "佐杜洛夫斯基的沙丘");
        assert!(
            first.name.contains("Jodorowskys.Dune.2013"),
            "{}",
            first.name
        );
        assert_eq!(first.format, "SUP");
        assert_eq!(
            first.languages,
            vec![
                "双语".to_string(),
                "简体".to_string(),
                "繁体".to_string(),
                "英语".to_string()
            ]
        );
        assert_eq!(first.origin, "其他来源");
        assert_eq!(first.size, "23079k");
        assert_eq!(first.downloads, "139");
        assert_eq!(first.date, "09-12 09:29");
        assert_eq!(first.uploader, "SubKing");
        assert_eq!(first.page_url, "https://subhd.me/a/gb6JIH");

        let second = &items[1];
        assert_eq!(second.id, "Yju3uE");
        assert_eq!(second.title, "沙丘：预言 第一季");
        assert_eq!(second.format, "ASS");
        assert_eq!(second.group, "衣柜字幕组");
        assert_eq!(second.size, "180k");
        assert_eq!(second.downloads, "190");
    }

    /// 卡片里没有 `/a/` 链接时不产出脏数据；id 只接受字母数字。
    #[test]
    fn ignores_cards_without_valid_subtitle_id() {
        let html = r#"<html><body>
            <div class="bg-white shadow-sm rounded-3 mb-4">
              <a href="/d/123">没有字幕 id</a>
            </div>
            <div class="bg-white shadow-sm rounded-3 mb-4">
              <a href="/a/../../etc/passwd">路径穿越</a>
            </div>
        </body></html>"#;
        assert!(parse_subhd_search(html, "https://subhd.me").is_empty());
    }

    /// JSON 错误文案提取：优先 msg，其次 message/error，最后兜底。
    #[test]
    fn reads_json_error_message() {
        let value = serde_json::json!({ "success": false, "msg": "验证失败" });
        assert_eq!(json_message(&value), "验证失败");
        let value = serde_json::json!({ "message": "Body is not valid JSON" });
        assert_eq!(json_message(&value), "Body is not valid JSON");
        assert_eq!(json_message(&serde_json::json!({})), "未知错误");
    }

    /// 中文关键词的路径编码不产生空格。
    #[test]
    fn encodes_query_segment() {
        let encoded = encode_segment("沙丘 2");
        assert!(!encoded.contains(' '), "{encoded}");
        assert!(encoded.starts_with('%'), "{encoded}");
    }
}
