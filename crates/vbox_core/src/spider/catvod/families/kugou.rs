//! `csp_Kugou` 原生实现（酷狗音乐榜单站：`https://www.kugou.com`，1 站）。
//!
//! 语义来自静态反编译：`_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/Kugou.java`
//! （357 行，`extends Spider`，0 skips，字符串经 `C0373a.a` 原样返回/URL 字面量）。
//!
//! 协议：
//! 1. 分类固定 3 个：`6666|0` 热门榜单 / `33162|1` 特色音乐榜 / `4681|2` 全球榜
//!    （`|` 前是榜单 id，`|` 后是 `.pc_rank_sidebar` 区块下标）。
//! 2. 列表：`GET {base}/yy/rank/home/1-{rankId}.html?from=rank`，
//!    取第 `index` 个 `.pc_rank_sidebar` 下的 `ul li a`：
//!    `href`→`vod_id`、`title`→`vod_name`（原实现 `m0(sel).d(index).g("ul li a")`）。
//! 3. 详情按 `vod_id` 的 URL 形态分三支：
//!    - `/mixsong/` 单曲页：`.songContent .audioName` 曲名、`.albumName a[title]` 专辑、
//!      `.singerName a[title]` 歌手；播放线路 `Qile`、单集 `歌手$id`；
//!    - `/songlist/` 歌单页：`#songs > strong` 名称、`#songs ul li a` 的 `title`/`href`
//!      逐曲为集（`#` 连接）；
//!    - 其它（榜单页）：`.pc_temp_title h3` 名称、`.rank_update` 备注、
//!      `.pc_temp_songlist ul li a.pc_temp_songname` 逐曲（`title` 优先，回退文本；
//!      `href` 为 mixsong 页），`#` 连接。
//! 4. 搜索：**依赖 Android WebView 拦截酷狗内部的签名接口**
//!    （`/v2/search/song?` 与 `/v1/search/special?`，见原实现 `shouldInterceptRequest`），
//!    桌面端无法等价复刻 → 返回稳定的 `catvod.host.unsupported`。
//! 5. 播放：原实现直接把分集地址当播放地址（`B.i.D(str2)` + 头部），这里同样
//!    返回 `{url, parse:0, header:{User-Agent}}`。
//!
//! 相对原实现的有意偏差（桌面端语义差异）：
//! - `ext` 为 http(s) 时整体覆盖基址（供 mock/镜像；默认仍写死真实主机）；
//! - 搜索返回明确的不可用错误，而不是静默空列表（WebView 分支在桌面端不存在）；
//! - `vod_remarks` 在 `VideoDetail` 无承载位，榜单备注被丢弃（列表中无该字段，故无影响）；
//! - 详情缺名称且无分集时返回 `catvod.result.parse`（原实现返回 `""`）。
//!
//! 真站现状（2026-11-11）：榜单页 `pc_rank_sidebar` / `pc_temp_songlist` /
//! `pc_temp_title` / `rank_update` 均在服务端 HTML 中；`/mixsong/{id}.html`
//! 的 `.songContent .audioName`/`.albumName`/`.singerName` 齐全 → 列表/详情可做真站验证。

use std::collections::HashMap;

use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule::join_url;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Kugou";
/// 原实现写死的站点基址。
pub const DEFAULT_BASE: &str = "https://www.kugou.com";
/// 原实现写死的 UA（Chrome 128 / Win64）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
/// 原实现的播放线路名。
const PLAY_FROM: &str = "Qile";

/// 原实现的固定分类（`|` 前榜单 id、`|` 后 sidebar 下标）。
pub const CATEGORIES: [(&str, &str); 3] = [
    ("6666|0", "热门榜单"),
    ("33162|1", "特色音乐榜"),
    ("4681|2", "全球榜"),
];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 基址、请求与解析工具
 * ============================================================ */

/// 基址：`ext` 为 http(s) 时覆盖（去尾 `/`），否则用写死的主机。
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

fn attr_of(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().trim().to_string()
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

fn fetch_text(cfg: &TvConfig, base: &str, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(base), 2)
}

/* ============================================================
 * 解析
 * ============================================================ */

/// 列表：取第 `index` 个 `.pc_rank_sidebar` 下的 `ul li a`。
fn parse_charts(html: &str, base: &str, index: usize) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let sidebars: Vec<ElementRef<'_>> = doc.select(&selector(".pc_rank_sidebar")).collect();
    let Some(sidebar) = sidebars.get(index) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for a in sidebar.select(&selector("ul li a")) {
        let href = attr_of(&a, "href");
        if href.is_empty() || href.starts_with("javascript") {
            continue;
        }
        let url = abs_url(base, &href);
        if url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let mut name = attr_of(&a, "title");
        if name.is_empty() {
            name = text_of(&a);
        }
        if name.is_empty() {
            continue;
        }
        out.push(VideoItem {
            id: url,
            name,
            cover: None,
            remark: None,
        });
    }
    out
}

/// 榜单页详情：名称、备注、逐曲（`标题` + mixsong 地址）。
fn parse_rank_detail(html: &str, base: &str) -> (String, String, Vec<Episode>) {
    let doc = Html::parse_document(html);
    let name = doc
        .select(&selector(".pc_temp_title h3"))
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();
    let remark = doc
        .select(&selector(".rank_update"))
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();
    let mut items = Vec::new();
    for row in doc.select(&selector(".pc_temp_songlist ul li")) {
        let Some(a) = row.select(&selector("a.pc_temp_songname")).next() else {
            continue;
        };
        let href = abs_url(base, &attr_of(&a, "href"));
        if href.is_empty() {
            continue;
        }
        let mut title = attr_of(&a, "title");
        if title.is_empty() {
            title = text_of(&a);
        }
        if title.is_empty() {
            title = format!("第{}首", items.len() + 1);
        }
        items.push(Episode { title, url: href });
    }
    (name, remark, items)
}

/// 单曲页详情：曲名、专辑、歌手、单集。
fn parse_mixsong(html: &str, id: &str) -> (String, String, Vec<Episode>) {
    let doc = Html::parse_document(html);
    let name = doc
        .select(&selector(".songContent .audioName"))
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();
    let album = doc
        .select(&selector(".songContent .albumName a"))
        .next()
        .map(|el| attr_of(&el, "title"))
        .unwrap_or_default();
    let singer = doc
        .select(&selector(".songContent .singerName a"))
        .next()
        .map(|el| attr_of(&el, "title"))
        .unwrap_or_default();
    let mut items = Vec::new();
    if !name.is_empty() || !singer.is_empty() {
        let title = if singer.is_empty() {
            name.clone()
        } else {
            singer.clone()
        };
        items.push(Episode {
            title,
            url: id.to_string(),
        });
    }
    (name, album, items)
}

/// 歌单页详情：名称、逐曲（`#songs ul li a`）。
fn parse_songlist(html: &str, base: &str) -> (String, Vec<Episode>) {
    let doc = Html::parse_document(html);
    let name = doc
        .select(&selector("#songs > strong"))
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();
    let mut items = Vec::new();
    for a in doc.select(&selector("#songs ul li a")) {
        let href = abs_url(base, &attr_of(&a, "href"));
        if href.is_empty() {
            continue;
        }
        let mut title = attr_of(&a, "title");
        if title.is_empty() {
            title = text_of(&a);
        }
        if title.is_empty() {
            title = format!("第{}首", items.len() + 1);
        }
        items.push(Episode { title, url: href });
    }
    (name, items)
}

/* ============================================================
 * 对外入口
 * ============================================================ */

pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 列表：`{base}/yy/rank/home/1-{rankId}.html?from=rank`。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, _page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let (rank, index) = split_cat(cat);
    let url = format!("{base}/yy/rank/home/1-{rank}.html?from=rank");
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_charts(&html, &base, index))
}

/// `cat` 形如 `6666|0`；缺省下标 0、缺省榜单 id `6666`。
fn split_cat(cat: &str) -> (String, usize) {
    let mut parts = cat.split('|');
    let rank = parts.next().unwrap_or_default().trim();
    let rank = if rank.is_empty() { "6666" } else { rank };
    let index = parts
        .next()
        .unwrap_or_default()
        .trim()
        .parse::<usize>()
        .unwrap_or(0);
    (rank.to_string(), index)
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = abs_url(&base, id);
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(CatvodError::new(ErrorClass::Parse, site, "csp_Kugou 详情地址为空").into_core());
    }
    let html = fetch_text(cfg, &base, &url)?;
    let (name, episodes) = if url.contains("/mixsong/") {
        let (name, _album, items) = parse_mixsong(&html, &url);
        (name, items)
    } else if url.contains("/songlist/") {
        let (name, items) = parse_songlist(&html, &base);
        (name, items)
    } else {
        let (name, _remark, items) = parse_rank_detail(&html, &base);
        (name, items)
    };
    if name.is_empty() && episodes.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "csp_Kugou 详情页缺少标题与曲目（站点改版或非酷狗页面？）",
        )
        .into_core());
    }
    Ok(VideoDetail {
        id: url,
        name,
        cover: None,
        desc: None,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: PLAY_FROM.to_string(),
            items: episodes,
        }],
    })
}

/// 搜索依赖 Android WebView 拦截酷狗内部签名接口，桌面端无等价实现。
pub fn search(_cfg: &TvConfig, site: &Site, _keyword: &str) -> Result<Vec<VideoItem>> {
    Err(CatvodError::new(
        ErrorClass::HostApi,
        site,
        "csp_Kugou 搜索依赖 Android WebView 拦截酷狗内部签名接口（/v2/search/song 与 /v1/search/special），桌面端无法等价实现",
    )
    .into_core())
}

/// 播放：原实现把分集地址直接当播放地址（带 UA 头）。
pub fn player_content(
    _cfg: &TvConfig,
    _site: &Site,
    _flag: &str,
    id: &str,
    _vip_flags: &[String],
) -> Result<Value> {
    if id.trim().is_empty() {
        return Ok(json!({"parse": 0, "playUrl": "", "url": ""}));
    }
    let mut header = HashMap::new();
    header.insert("User-Agent", UA);
    Ok(json!({
        "url": id.trim(),
        "parse": 0,
        "playUrl": "",
        "header": header,
    }))
}

/* ============================================================
 * 测试
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    const CHART_HTML: &str = r#"
<div class="pc_rank_sidebar">
  <h3 data-index="0"><a href="javascript:;" title="热门榜单">热门榜单</a></h3>
  <ul>
    <li><a title="酷狗飙升榜" href="https://www.kugou.com/yy/rank/home/1-6666.html?from=rank">酷狗飙升榜</a></li>
  </ul>
</div>
<div class="pc_rank_sidebar">
  <h3 data-index="1"><a href="javascript:;" title="特色音乐榜">特色音乐榜</a></h3>
  <ul>
    <li><a title="酷狗TOP500" href="https://www.kugou.com/yy/rank/home/1-8888.html?from=rank">酷狗TOP500</a></li>
    <li><a title="内地榜" href="https://www.kugou.com/yy/rank/home/1-31308.html?from=rank">内地榜</a></li>
  </ul>
</div>
"#;

    const RANK_HTML: &str = r#"
<h3 class="pc_temp_title"><h3 class="pc_temp_title"><h3>酷狗飙升榜</h3></h3></h3>
<div class="pc_temp_title"><h3>酷狗飙升榜</h3></div>
<div class="rank_update">更新于 2026-11-11</div>
<div class="pc_temp_songlist"><ul>
  <li title="张韶涵、虚环 - 回旋" data-eid="g9b2v34b">
    <a href="https://www.kugou.com/mixsong/g9b2v34b.html" class="pc_temp_songname" title="张韶涵、虚环 - 回旋">回旋</a>
  </li>
</ul></div>
"#;

    const MIX_HTML: &str = r#"
<div class="songContent">
  <span class="audioName"> 张韶涵、虚环 - 回旋 </span>
  <p class="albumName"><a title="">回旋</a></p>
  <p class="singerName"><a title="张韶涵">张韶涵</a></p>
</div>
"#;

    const SONGLIST_HTML: &str = r#"
<div id="songs">
  <strong>精选歌单</strong>
  <ul>
    <li><a title="夜曲" href="https://www.kugou.com/mixsong/abc.html">夜曲</a></li>
    <li><a title="晴天" href="https://www.kugou.com/mixsong/def.html">晴天</a></li>
  </ul>
</div>
"#;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Kugou"));
        assert!(handles(" CSP_KUGOU "));
        assert!(!handles("csp_KugouX"));
    }

    #[test]
    fn categories_are_fixed_three() {
        let c = categories();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].type_id, "6666|0");
        assert_eq!(c[1].type_name, "特色音乐榜");
    }

    #[test]
    fn chart_url_uses_rank_id_and_index() {
        assert_eq!(split_cat("6666|0"), ("6666".to_string(), 0));
        assert_eq!(split_cat("4681|2"), ("4681".to_string(), 2));
        assert_eq!(split_cat(""), ("6666".to_string(), 0));
        assert_eq!(split_cat("8888"), ("8888".to_string(), 0));
    }

    #[test]
    fn charts_pick_matching_sidebar_block() {
        let first = parse_charts(CHART_HTML, "https://www.kugou.com", 0);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].name, "酷狗飙升榜");
        assert!(first[0].id.ends_with("/1-6666.html?from=rank"));

        let second = parse_charts(CHART_HTML, "https://www.kugou.com", 1);
        assert_eq!(second.len(), 2);
        assert_eq!(second[1].name, "内地榜");
        assert!(parse_charts(CHART_HTML, "https://www.kugou.com", 9).is_empty());
    }

    #[test]
    fn rank_detail_extracts_name_and_songs() {
        let (name, remark, items) = parse_rank_detail(RANK_HTML, "https://www.kugou.com");
        assert_eq!(name, "酷狗飙升榜");
        assert_eq!(remark, "更新于 2026-11-11");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "张韶涵、虚环 - 回旋");
        assert!(items[0].url.ends_with("/mixsong/g9b2v34b.html"));
    }

    #[test]
    fn mixsong_has_single_episode() {
        let (name, album, items) = parse_mixsong(MIX_HTML, "https://www.kugou.com/mixsong/g9b2v34b.html");
        assert_eq!(name, "张韶涵、虚环 - 回旋");
        assert_eq!(album, "");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "张韶涵");
        assert!(items[0].url.ends_with("/mixsong/g9b2v34b.html"));
    }

    #[test]
    fn songlist_collects_songs() {
        let (name, items) = parse_songlist(SONGLIST_HTML, "https://www.kugou.com");
        assert_eq!(name, "精选歌单");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].title, "晴天");
    }

    #[test]
    fn search_reports_webview_dependency() {
        let site = Site {
            key: "kugou".into(),
            name: "酷狗音乐".into(),
            api: API.into(),
            ..Default::default()
        };
        let err = search(&TvConfig::default(), &site, "周杰伦").unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("catvod.host.unsupported"), "{msg}");
    }

    #[test]
    fn player_returns_direct_url_with_ua() {
        let site = Site::default();
        let v = player_content(&TvConfig::default(), &site, "", "https://www.kugou.com/mixsong/x.html", &[]).unwrap();
        assert_eq!(v["parse"], 0);
        assert!(v["url"].as_str().unwrap().ends_with("x.html"));
        assert!(v["header"]["User-Agent"].as_str().unwrap().contains("Chrome"));
    }
}
