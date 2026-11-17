//! `csp_Mp4Mov` 原生实现（MP4 电影下载站：`https://mp4.z6.net.cn`，1 站）。
//!
//! 语义来自静态反编译：`_rea/decompiled/5fc50e17-…/deobf/com/github/catvod/spider/Mp4Mov.java`
//! （265 行，`extends Spider`，0 skips，无字符串混淆）。基址**写死**
//! `https://mp4.z6.net.cn`，`init(Context,String)` 里 `ext` 非空则整体覆盖基址。
//!
//! 协议：
//! 1. 分类固定 9 个：`1..9` = 动作片/科幻片/爱情片/喜剧片/恐怖片/战争片/剧情片/纪录片/动画片。
//! 2. 列表：`GET {base}/list/{cat}-{page}.html`，行 `div#list_all ul>li`
//!    （`cat == 99` 时改走 `div#list_dy ul>li`：只有标题与时间、无封面）。
//!    标题取 `h2`（原实现再按 `《…》` 拆出片名与备注），封面取
//!    `img[data-original]`（原站常带前导空格，需要 trim），备注取 `.update_time`；
//!    id 取行内首个 `a[href]`（形如 `/detail/107699.html`）。
//! 3. 详情：`GET {base}{id}`，标题 `div.article-header h1`、封面 `div.pic > img[src]`、
//!    分类 `ul.breadcrumb li` 第 2 项；`ul.down-list` 每个分组只保留
//!    `a[href^=magnet]` / `a[href^=ed2k]` 的条目（原实现用 `li:has(...)` 过滤），
//!    分组名 `列表N`、条目 `标题$地址`、`#` 分集、`$$$` 分线路；分类文本含
//!    「电视剧」时分组内条目倒序。
//! 4. 搜索：`GET {base}/search/?wd={kw}&p={page}&t={t}`，其中
//!    `t = Base64(AES-128-ECB/PKCS5(秒级时间戳, key="7230789078907890"))`（no-wrap）。
//! 5. 播放：直接回 `{url: 分集地址, parse: 0, playUrl: ""}`（原实现外挂弹幕，这里不实现）。
//!
//! 相对原实现的有意偏差（桌面端语义差异，均已注释）：
//! - `ext` 整体为 http(s) 时覆盖基址（供 mock/自建镜像；默认仍写死真实主机）；
//! - 详情缺标题且无分集时返回稳定的 `catvod.result.parse`（原实现返回 `""`）；
//! - `parse`/`pagecount` 不承载（VBox 列表按 `page` 请求，不做总页数预判）；
//! - 弹幕字段不实现（桌面端本就不接 `Proxy.getUrl()` 弹幕）。
//!
//! 真站现状（2026-11-11）：`/list/1-1.html` 可用（10 行/页，标题/`data-original`/
//! `.update_time` 齐全）；`/detail/{id}.html` 可用（`div.article-header h1`、
//! `ul.breadcrumb`、`ul.down-list` 内同时含 `jianpian://` 与 `magnet:` 条目）；
//! `/search/` 端点当前返回站点自身 SQL 错误页（上游故障）→ 搜索只能 mock 验收。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule::join_url;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Mp4Mov";
/// 原实现写死的站点基址。
pub const DEFAULT_BASE: &str = "https://mp4.z6.net.cn";
/// 原实现写死的 UA（Chrome 94 / Win64）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.54 Safari/537.36";
/// 搜索签名用的 AES-128 key（原实现 `encrypt(valueOf, "7230789078907890")`）。
const AES_KEY: &[u8] = b"7230789078907890";

/// 原实现的固定分类。
pub const CATEGORIES: [(&str, &str); 9] = [
    ("1", "动作片"),
    ("2", "科幻片"),
    ("3", "爱情片"),
    ("4", "喜剧片"),
    ("5", "恐怖片"),
    ("6", "战争片"),
    ("7", "剧情片"),
    ("8", "纪录片"),
    ("9", "动画片"),
];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 基址与请求
 * ============================================================ */

/// 基址：`ext` 为 http(s) 时覆盖，否则用写死的主机。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let first = ext
        .split("$$$")
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/');
    if first.starts_with("http://") || first.starts_with("https://") {
        first.to_string()
    } else {
        DEFAULT_BASE.to_string()
    }
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), format!("{base}/"));
    h
}

fn selector(text: &str) -> Selector {
    Selector::parse(text).expect("valid css selector")
}

fn text_of(el: &ElementRef<'_>) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn abs_url(base: &str, raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else if let Some(rest) = s.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        join_url(base, s)
    }
}

/// 原实现的标题拆分：`xxx《片名》备注` → (`片名`, `备注`)。
fn split_title(title: &str) -> (String, String) {
    if title.contains('《') && title.contains('》') {
        let name = title
            .split('《')
            .nth(1)
            .unwrap_or(title)
            .split('》')
            .next()
            .unwrap_or(title)
            .trim()
            .to_string();
        let remark = title.split('》').nth(1).unwrap_or("").trim().to_string();
        if !name.is_empty() {
            return (name, remark);
        }
    }
    (title.trim().to_string(), String::new())
}

/* ============================================================
 * 列表
 * ============================================================ */

fn parse_rows(html: &str, base: &str, list_dy: bool) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = if list_dy {
        selector("div#list_dy ul>li")
    } else {
        selector("div#list_all ul>li")
    };
    let h2_sel = selector("h2");
    let a_sel = selector("a[href]");
    let img_sel = selector("img");
    let update_sel = selector(".update_time");
    let time_sel = selector(".time");

    let mut items = Vec::new();
    for row in doc.select(&row_sel) {
        let link = match row.select(&a_sel).next() {
            Some(a) => a.value().attr("href").unwrap_or("").trim().to_string(),
            None => continue,
        };
        if link.is_empty() {
            continue;
        }
        let raw_title = if list_dy {
            row.select(&a_sel)
                .next()
                .map(|a| text_of(&a))
                .unwrap_or_default()
        } else {
            row.select(&h2_sel)
                .next()
                .map(|h| text_of(&h))
                .or_else(|| row.select(&a_sel).next().map(|a| text_of(&a)))
                .unwrap_or_default()
        };
        if raw_title.is_empty() {
            continue;
        }
        let (name, remark_from_title) = split_title(&raw_title);
        let cover = row
            .select(&img_sel)
            .next()
            .and_then(|img| {
                img.value()
                    .attr("data-original")
                    .or_else(|| img.value().attr("src"))
                    .map(str::to_string)
            })
            .map(|raw| abs_url(base, &raw))
            .filter(|s| !s.is_empty());
        let remark = row
            .select(&update_sel)
            .next()
            .map(|e| text_of(&e))
            .filter(|s| !s.is_empty())
            .or_else(|| {
                row.select(&time_sel)
                    .next()
                    .map(|e| text_of(&e))
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| {
                if remark_from_title.is_empty() {
                    None
                } else {
                    Some(remark_from_title)
                }
            });
        items.push(VideoItem {
            id: link,
            name,
            cover,
            remark,
        });
    }
    items
}

pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: id.to_string(),
            type_name: name.to_string(),
        })
        .collect()
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let cat = if cat.trim().is_empty() { "1" } else { cat.trim() };
    let page = page.max(1);
    let url = format!("{base}/list/{cat}-{page}.html");
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(&base), 1)?;
    Ok(parse_rows(&html, &base, cat == "99"))
}

/* ============================================================
 * 详情
 * ============================================================ */

fn parse_detail(site: &Site, id: &str, base: &str, html: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);
    let title = ["div.article-header h1", "h1"]
        .iter()
        .find_map(|sel| doc.select(&selector(sel)).next().map(|e| text_of(&e)))
        .unwrap_or_default();
    let cover = ["div.pic > img", "div.pic img"]
        .iter()
        .find_map(|sel| doc.select(&selector(sel)).next())
        .and_then(|img| {
            img.value()
                .attr("src")
                .or_else(|| img.value().attr("data-original"))
                .map(|raw| abs_url(base, raw))
        })
        .filter(|s| !s.is_empty());
    let category = doc
        .select(&selector("ul.breadcrumb li"))
        .nth(1)
        .map(|e| text_of(&e))
        .unwrap_or_default();
    let reverse = category.contains("电视剧");

    let mut episodes = Vec::new();
    for (idx, group) in doc.select(&selector("ul.down-list")).enumerate() {
        let mut links: Vec<ElementRef<'_>> = group
            .select(&selector("a[href]"))
            .filter(|a| {
                let href = a.value().attr("href").unwrap_or("");
                href.starts_with("magnet:") || href.starts_with("ed2k:")
            })
            .collect();
        if reverse {
            links.reverse();
        }
        let mut items = Vec::new();
        for (i, a) in links.iter().enumerate() {
            let href = a.value().attr("href").unwrap_or("").trim().to_string();
            if href.is_empty() {
                continue;
            }
            let label = a
                .value()
                .attr("title")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| text_of(a));
            let label = if label.is_empty() {
                format!("第{}集", i + 1)
            } else {
                label
            };
            items.push(Episode {
                title: label,
                url: href,
            });
        }
        if items.is_empty() {
            continue;
        }
        episodes.push(EpisodeGroup {
            group: format!("列表{}", idx + 1),
            items,
        });
    }

    if title.is_empty() && episodes.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("详情页缺少标题与下载列表: {id}"),
        )
        .into_core());
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name: title,
        cover,
        desc: None,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes,
    })
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else if id.starts_with('/') {
        format!("{base}{id}")
    } else {
        format!("{base}/{id}")
    };
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(&base), 1)?;
    parse_detail(site, id, &base, &html)
}

/* ============================================================
 * 搜索（AES 时间戳签名）
 * ============================================================ */

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 原实现 `encrypt(str, "7230789078907890")`：AES-128-ECB/PKCS5 + Base64(NO_WRAP)。
fn aes_128_ecb_pkcs5_b64(plain: &str) -> String {
    use aes::cipher::{BlockEncrypt, KeyInit};
    let cipher = aes::Aes128::new_from_slice(AES_KEY).expect("AES key 长度固定 16 字节");
    let mut buf = plain.as_bytes().to_vec();
    let pad = 16 - (buf.len() % 16);
    buf.extend(std::iter::repeat(pad as u8).take(pad));
    for chunk in buf.chunks_mut(16) {
        let block = aes::Block::from_mut_slice(chunk);
        cipher.encrypt_block(block);
    }
    base64::engine::general_purpose::STANDARD.encode(&buf)
}

fn search_url(base: &str, keyword: &str, page: u32, ts: u64) -> String {
    let kw = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let token = aes_128_ecb_pkcs5_b64(&ts.to_string());
    let token = percent_encoding::utf8_percent_encode(&token, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    format!("{base}/search/?wd={kw}&p={}&t={token}", page.max(1))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = search_url(&base, keyword, 1, now_seconds());
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(&base), 1)?;
    Ok(parse_rows(&html, &base, false))
}

/* ============================================================
 * 播放
 * ============================================================ */

pub fn player_content(
    _cfg: &TvConfig,
    _site: &Site,
    _flag: &str,
    id: &str,
) -> Result<Value> {
    // 原实现：magnet/ed2k 直出，`parse=0`；弹幕字段（`Proxy.getUrl()?do=diydanmu`）桌面端不实现。
    Ok(json!({
        "url": id,
        "parse": 0,
        "playUrl": "",
    }))
}

/* ============================================================
 * 单测
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Option<&str>) -> Site {
        let mut s = Site::default();
        s.key = "mp4mov".to_string();
        s.name = "MP4电影".to_string();
        s.api = API.to_string();
        if let Some(ext) = ext {
            s.ext = Some(json!(ext));
        }
        s
    }

    #[test]
    fn handles_api_case_insensitively() {
        assert!(handles("csp_Mp4Mov"));
        assert!(handles(" CSP_MP4MOV "));
        assert!(!handles("csp_SixV"));
    }

    #[test]
    fn base_falls_back_and_accepts_ext() {
        assert_eq!(base_of(&site(None)), DEFAULT_BASE);
        assert_eq!(base_of(&site(Some("https://mirror.example/"))), "https://mirror.example");
        assert_eq!(base_of(&site(Some("${base}"))), DEFAULT_BASE);
    }

    #[test]
    fn title_split_extracts_name_and_remark() {
        assert_eq!(
            split_title("2010年泰国经典动作片《拳霸3》蓝光泰语中字迅雷下载"),
            ("拳霸3".to_string(), "蓝光泰语中字迅雷下载".to_string())
        );
        assert_eq!(split_title("无书名号"), ("无书名号".to_string(), String::new()));
    }

    #[test]
    fn parses_list_rows() {
        let html = r#"<div id="list_all"><ul>
          <li><a href="/detail/107699.html"><img data-original=" https://img.example/a.jpg "></a>
            <div class="text_info"><h2><a href="/detail/107699.html">动作片《拳霸3》蓝光中字</a></h2>
            <p><span class="update_time">2026-09-14</span></p></div></li>
          <li><a href="/detail/2.html"><img src="/static/2.jpg"></a>
            <div class="text_info"><h2>无书名号影片</h2><p><span class="time">2024-01-02</span></p></div></li>
        </ul></div>"#;
        let rows = parse_rows(html, "https://mp4.test", false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "/detail/107699.html");
        assert_eq!(rows[0].name, "拳霸3");
        assert_eq!(rows[0].cover.as_deref(), Some("https://img.example/a.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("2026-09-14"));
        assert_eq!(rows[1].name, "无书名号影片");
        assert_eq!(rows[1].cover.as_deref(), Some("https://mp4.test/static/2.jpg"));
        assert_eq!(rows[1].remark.as_deref(), Some("2024-01-02"));
    }

    #[test]
    fn parses_detail_groups_with_magnet_and_ed2k_only() {
        let html = r#"<ul class="breadcrumb cclear"><li>首页</li><li><a>动作</a></li><li>下载详情</li></ul>
          <div class="article-header"><h1>拳霸3</h1><div class="pic"><img src="https://img.example/p.jpg"></div></div>
          <ul class="panel down-list thunder1">
            <li><a href="jianpian://pathtype=url" title="忽略">忽略</a></li>
            <li><a href="magnet:?xt=urn:btih:aa" title="蓝光中字">蓝光</a></li>
            <li><a href="ed2k://|file|x.mp4|1|/|">电驴</a></li>
          </ul>"#;
        let d = parse_detail(&site(None), "/detail/1.html", "https://mp4.test", html).unwrap();
        assert_eq!(d.name, "拳霸3");
        assert_eq!(d.cover.as_deref(), Some("https://img.example/p.jpg"));
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "列表1");
        assert_eq!(d.episodes[0].items.len(), 2);
        assert_eq!(d.episodes[0].items[0].title, "蓝光中字");
        assert!(d.episodes[0].items[0].url.starts_with("magnet:"));
        assert_eq!(d.episodes[0].items[1].title, "电驴");
    }

    #[test]
    fn aes_signature_matches_java_side() {
        assert_eq!(aes_128_ecb_pkcs5_b64("1700000000"), "CoQCaPngXpirdHvsLm5T5A==");
    }

    #[test]
    fn search_url_carries_encoded_keyword_and_token() {
        let url = search_url("https://mp4.test", "拳霸", 1, 1_700_000_000);
        assert!(url.starts_with("https://mp4.test/search/?wd=%E6%8B%B3%E9%9C%B8&p=1&t="));
        assert!(url.contains("CoQCaPngXpirdHvsLm5T5A"));
    }
}
