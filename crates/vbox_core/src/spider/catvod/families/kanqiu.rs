//! `csp_Kanqiu` 原生实现（88看球：体育直播聚合站，1 站）。
//!
//! 语义来自静态反编译：`_rea/decompiled/5d982c72-_/deobf/com/github/catvod/spider/Kanqiu.java`
//! （121 行，`extends Spider`，0 skips，字符串常量走 `prL.d()` XOR，已用
//! `_rea/tools/decode-prl.mjs` 全部解出）。基址**写死** `http://www.88kanqiu.one`，
//! `init(Context,String)` 里若 `ext` 非空则覆盖基址。
//!
//! 协议：
//! 1. 分类固定 4 个：`""`=全部直播、`1`=篮球直播、`8`=足球直播、`29`=其他直播。
//! 2. 列表：`GET {base}`（全部直播）或 `GET {base}/match/{cat}/live`（其余分类）。
//!    行 `li.list-group-item`：直播行带 `a.btn.btn-primary[href]`（形如 `/live/{id}/play`），
//!    标题取 `.row.d-none` 文本（原实现取行文本兜底），封面取行内首个 `img`
//!    的 `src`（空则 `data-src`，相对路径补基址，仍空则占位图
//!    `https://qlql.link/b/日期.jpg`），备注取按钮文本（「直播中」「已结束」）。
//! 3. 详情：`GET {id}-url`（id 即列表给出的绝对播放页地址），返回
//!    `{"links":[{"name":…,"url":…}]}`；原实现把 `name$url` 用 `#` 连接、
//!    播放组名固定 `Qile`。
//! 4. 播放：直接返回 `{parse:0, url:{id}, header:{UA}}`（直播流地址即分集 url）。
//! 5. 搜索：原实现**未覆写** `searchContent`（继承默认空结果），保持一致。
//!
//! 相对原实现的有意偏差（桌面端语义差异，均已注释）：
//! - `ext` 首段为 http(s) 时覆盖基址（供 mock/自建镜像；默认仍写死真实主机）；
//! - 「全部直播」分类 id 为空串时请求站点首页；其余分类走 `/match/{cat}/live`
//!   （原实现在筛选值为空时拼 `{base}{cat}`，对非空 id 会产生非法 URL，这里改为
//!   站点真实路由）；
//! - 分类的筛选组（`cateId` 下的 NBA/CBA/英超…）不接收 —— VBox `Site` 无筛选承载位；
//! - 行内没有直播按钮（按钮是 `btn-default` 的历史/待开始场次）时退化为行内首个
//!   `a[href]`，避免原实现把 id 拼成基址导致重复项；两者都没有则跳过该行；
//! - 详情缺 `links` 时返回稳定的 `catvod.result.parse`（原实现会抛出空指针）。

use std::collections::HashMap;

use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Kanqiu";
/// 原实现写死的站点基址。
pub const DEFAULT_BASE: &str = "http://www.88kanqiu.one";
/// 原实现写死的 UA（Chrome 109 / Win64）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36";
/// 原实现写死的封面占位（`prL.d("0E043E1E…")` 解出 `https://qlql.link/b/日期.jpg`）。
const PLACEHOLDER_COVER: &str = "https://qlql.link/b/日期.jpg";
/// 原实现写死的播放组名（`prL.d("3719260B")` → `Qile`）。
const PLAY_FROM: &str = "Qile";

/// 原实现的固定分类（`type_id` 为空串表示站点首页）。
pub const CATEGORIES: [(&str, &str); 4] = [
    ("", "全部直播"),
    ("1", "篮球直播"),
    ("8", "足球直播"),
    ("29", "其他直播"),
];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 基址与请求
 * ============================================================ */

/// 基址：`ext` 首段为 http(s) 时覆盖，否则用写死的主机。
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

fn headers() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), UA.to_string());
    map
}

fn fetch_text(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(), 1)
}

/* ============================================================
 * 分类 / 列表
 * ============================================================ */

/// 固定 4 个分类（保持原实现顺序）。
pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(type_id, type_name)| Category {
            type_id: (*type_id).to_string(),
            type_name: (*type_name).to_string(),
        })
        .collect()
}

/// 列表地址：空分类 → 站点首页；其余 → `/match/{cat}/live`。
fn list_url(base: &str, cat: &str) -> String {
    let cat = cat.trim();
    if cat.is_empty() {
        base.to_string()
    } else {
        format!("{base}/match/{cat}/live")
    }
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, _page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, cat);
    let html = fetch_text(cfg, &url)?;
    Ok(parse_rows(&html, &base))
}

fn selector(rule: &str) -> Selector {
    Selector::parse(rule).expect("静态选择器必须可解析")
}

fn attr(el: &ElementRef, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().trim().to_string()
}

fn text_of(el: &ElementRef) -> String {
    el.text().collect::<String>()
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn abs_url(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }
    let joined = crate::html_rule::join_url(base, href);
    (!joined.is_empty()).then_some(joined)
}

/// 解析列表页：`li.list-group-item` 行；优先直播按钮 `.btn.btn-primary`。
pub fn parse_rows(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = selector("li.list-group-item");
    let live_sel = selector("a.btn.btn-primary");
    let any_link_sel = selector("a[href]");
    let name_sel = selector(".row.d-none");
    let img_sel = selector("img");

    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let anchor = row
            .select(&live_sel)
            .next()
            .or_else(|| row.select(&any_link_sel).next());
        let Some(anchor) = anchor else { continue };
        let Some(id) = abs_url(base, &attr(&anchor, "href")) else {
            continue;
        };
        let mut name = row
            .select(&name_sel)
            .next()
            .map(|el| collapse(&text_of(&el)))
            .unwrap_or_default();
        if name.is_empty() {
            name = collapse(&text_of(&row));
        }
        let remark = collapse(&text_of(&anchor));
        let cover = row
            .select(&img_sel)
            .next()
            .and_then(|img| {
                let src = attr(&img, "src");
                let src = if src.is_empty() { attr(&img, "data-src") } else { src };
                abs_url(base, &src)
            })
            .unwrap_or_else(|| PLACEHOLDER_COVER.to_string());

        out.push(VideoItem {
            id,
            name,
            cover: Some(cover),
            remark: (!remark.is_empty()).then_some(remark),
        });
    }
    out
}

/* ============================================================
 * 详情 / 播放 / 搜索
 * ============================================================ */

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    let base = base_of(site);
    if id.is_empty() || id.trim_end_matches('/') == base {
        return Err(CatvodError::new(ErrorClass::Parse, site, "比赛尚未开始").into_core());
    }
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        format!("{}-url", id.trim_end_matches('/'))
    } else {
        format!("{base}/{}-url", id.trim_start_matches('/'))
    };
    let body = fetch_text(cfg, &url)?;
    parse_detail(site, id, &body)
}

/// 解析 `{"links":[{"name":…,"url":…}]}`。
pub fn parse_detail(site: &Site, id: &str, body: &str) -> Result<VideoDetail> {
    let value: Value = serde_json::from_str(body).map_err(|err| {
        CatvodError::new(ErrorClass::Parse, site, format!("详情响应不是 JSON: {err}")).into_core()
    })?;
    let links = value
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut items = Vec::new();
    for link in &links {
        let name = link
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let url = link
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if url.is_empty() {
            continue;
        }
        let title = if name.is_empty() {
            format!("第{}集", items.len() + 1)
        } else {
            name
        };
        items.push(Episode { title, url });
    }

    if items.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情没有可播放地址（比赛可能尚未开始或已结束）",
        )
        .into_core());
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name: id.to_string(),
        cover: None,
        desc: None,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: PLAY_FROM.to_string(),
            items,
        }],
    })
}

/// 原实现未覆写 `searchContent` → 默认空结果。
pub fn search(_cfg: &TvConfig, _site: &Site, _keyword: &str) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// 直播流地址即详情里给出的 `url`；播放组名 `Qile`、`parse=0`。
pub fn player_content(_cfg: &TvConfig, _site: &Site, _flag: &str, id: &str) -> Result<Value> {
    Ok(json!({
        "parse": 0,
        "url": id,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

/* ============================================================
 * 单测
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            key: "kanqiu".to_string(),
            name: "88看球".to_string(),
            api: API.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn handles_kanqiu_case_insensitive() {
        assert!(handles("csp_Kanqiu"));
        assert!(handles(" CSP_KANQIU "));
        assert!(!handles("csp_KanqiuX"));
        assert!(!handles("csp_Kanqiu2"));
        assert_eq!(super::API, "csp_Kanqiu");
    }

    #[test]
    fn categories_are_fixed_four() {
        let cats = categories();
        assert_eq!(cats.len(), 4);
        assert_eq!(cats[0].type_id, "");
        assert_eq!(cats[0].type_name, "全部直播");
        assert_eq!(cats[1].type_id, "1");
        assert_eq!(cats[1].type_name, "篮球直播");
        assert_eq!(cats[2].type_id, "8");
        assert_eq!(cats[3].type_id, "29");
    }

    #[test]
    fn list_url_uses_real_route() {
        assert_eq!(list_url("http://a.test", ""), "http://a.test");
        assert_eq!(
            list_url("http://a.test", "1"),
            "http://a.test/match/1/live"
        );
        assert_eq!(
            list_url("http://a.test", " 29 "),
            "http://a.test/match/29/live"
        );
    }

    #[test]
    fn parse_rows_reads_live_item() {
        let html = r#"
        <ul class="list-group">
          <li class="list-group-item live-list-group-time"><span class="date"></span>2026-11-11</li>
          <li class="list-group-item group-game-item">
            <div class="row d-none d-lg-flex">
              <div class="col-md-1 category-game-time">09:00</div>
              <div class="col-md-3"><span class="game-type">NBA</span></div>
              <div class="col-xs-1"><img src="/static/img/default.png" data-src="https://cdn.test/nba.png"></div>
              <div class="col-md-3 pay-btn"><a class="btn btn-primary" href="/live/130986/play">直播中</a></div>
            </div>
          </li>
        </ul>"#;
        let rows = parse_rows(html, "http://www.88kanqiu.one");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "http://www.88kanqiu.one/live/130986/play");
        assert!(rows[0].name.contains("09:00"));
        assert_eq!(rows[0].cover.as_deref(), Some("http://www.88kanqiu.one/static/img/default.png"));
        assert_eq!(rows[0].remark.as_deref(), Some("直播中"));
    }

    #[test]
    fn parse_rows_skips_rows_without_link() {
        let html = r#"
        <ul class="list-group">
          <li class="list-group-item live-list-group-time"><span class="date"></span>2026-11-11</li>
          <li class="list-group-item group-game-item"><div class="row d-none">待开始</div></li>
        </ul>"#;
        assert!(parse_rows(html, "http://a.test").is_empty());
    }

    #[test]
    fn parse_detail_builds_qile_group() {
        let body = r#"{"links":[{"name":"高清1","url":"http://cdn.test/live1.m3u8"},{"name":"","url":"http://cdn.test/live2.m3u8"}]}"#;
        let detail = parse_detail(&site(), "http://a.test/live/1/play", body).expect("detail");
        assert_eq!(detail.episodes.len(), 1);
        assert_eq!(detail.episodes[0].group, "Qile");
        assert_eq!(detail.episodes[0].items.len(), 2);
        assert_eq!(detail.episodes[0].items[0].title, "高清1");
        assert_eq!(detail.episodes[0].items[1].title, "第2集");
        assert_eq!(detail.episodes[0].items[0].url, "http://cdn.test/live1.m3u8");
    }

    #[test]
    fn parse_detail_rejects_empty_links() {
        let err = parse_detail(&site(), "http://a.test/live/1/play", r#"{"links":[]}"#).unwrap_err();
        let text = format!("{err:?}");
        assert!(text.contains("catvod.result.parse"), "unexpected: {text}");
    }

    #[test]
    fn player_passthrough() {
        let cfg = TvConfig::default();
        let value = player_content(&cfg, &site(), "qile", "http://cdn.test/live1.m3u8").unwrap();
        assert_eq!(value["parse"], 0);
        assert_eq!(value["url"], "http://cdn.test/live1.m3u8");
        assert!(value["header"]["User-Agent"].as_str().unwrap().contains("Chrome"));
    }

    #[test]
    fn search_is_empty_like_original() {
        let cfg = TvConfig::default();
        assert!(search(&cfg, &site(), "anything").unwrap().is_empty());
    }
}
