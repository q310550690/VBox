//! `csp_SixV` 原生实现（MacCMS 变体，第二个垂直切片）。
//!
//! 语义来自静态反编译：`_rea/decompiled/69dadb9a-Yoursmile/deobf/com/github/catvod/spider/SixV.java`
//! （193 行，去混淆后可直接读）。该类 `extends Spider`，全部逻辑都在
//! `init` / `homeContent` / `categoryContent` / `detailContent` / `searchContent` / `playerContent`
//! 里，无宿主私有 API 依赖，因此可以 100% 原生重写：
//!
//! 1. `init(ctx, ext)`：`ext` 即站点基址（去掉尾部 `/`，例如 `https://www.6vdy.org`）。
//! 2. `homeContent`：固定 12 个分类（`dianshiju/guoju` … `donghuapian`），不发请求。
//! 3. `categoryContent(typeId, page)`：
//!    `{base}/{typeId}`（page≠1 时追加 `/index_{page}.html`）；
//!    UA = Chrome 39 (WOW64)、Referer = `{base}/`；
//!    解析 `#post_container .post_hover`，每行取第一个 `[class=zoom]` 的
//!    `href`/`title`、`img` 的 `src`，`[rel=category tag]` 文本作备注。
//! 4. `detailContent(id)`：`{base}{id}`；`#post_content` 下每个 `table` 为一组
//!    magnet 链接（`文本$magnet:…`，`#` 分集、`$$$` 分线路，组名 `magnet1`…）；
//!    `.context` 拼 html 后用「◎类　　别　… <br>」式中文标签正则抽元数据；
//!    标题 `.article_container > h1`，封面 `#post_content img[src]`。
//! 5. `searchContent(word)`：`POST {base}/e/search/index.php`
//!    （`show=title&tempid=1&tbname=article&mid=1&dopost=search&submit=&keyboard=…`），
//!    解析 `#post_container [class=zoom]`。
//! 6. `playerContent`：`{"parse":0,"header":"","playUrl":"","url":id}`，即 magnet 直出。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;

use crate::error::Result;
use crate::html_rule;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_SixV";

/// 安卓端 UA（`Chrome/39.0.2171.71`，Windows 7 WOW64）。
const UA: &str = "Mozilla/5.0 (Windows NT 6.1; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/39.0.2171.71 Safari/537.36";

/// 固定首页分类（与 `homeContent` 中两个数组一一对应）。
const CATEGORIES: [(&str, &str); 12] = [
    ("dianshiju/guoju", "国剧"),
    ("dianshiju/rihanju", "日韩剧"),
    ("dianshiju/oumeiju", "欧美剧"),
    ("xijupian", "喜剧片"),
    ("dongzuopian", "动作片"),
    ("aiqingpian", "爱情片"),
    ("kehuanpian", "科幻片"),
    ("kongbupian", "恐怖片"),
    ("juqingpian", "剧情片"),
    ("zhanzhengpian", "战争片"),
    ("jilupian", "纪录片"),
    ("donghuapian", "动画片"),
];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 选择器
 * ============================================================ */

fn list_row_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("#post_container .post_hover").expect("列表行选择器"))
}

fn zoom_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("[class=zoom]").expect("zoom 选择器"))
}

fn img_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("img").expect("img 选择器"))
}

fn rel_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("[rel]").expect("rel 选择器"))
}

fn content_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("#post_content").expect("#post_content 选择器"))
}

fn table_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("table").expect("table 选择器"))
}

fn anchor_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("a").expect("a 选择器"))
}

fn context_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse(".context").expect(".context 选择器"))
}

fn h1_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse(".article_container > h1").expect("标题选择器"))
}

fn content_img_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("#post_content img").expect("详情封面选择器"))
}

fn search_zoom_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| {
        Selector::parse("#post_container [class=zoom]").expect("搜索行选择器")
    })
}

/* ============================================================
 * 入口
 * ============================================================ */

/// 首页分类（固定 12 个，不发请求）。
pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 分类列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site)?;
    let url = if page <= 1 {
        format!("{base}/{cat}")
    } else {
        format!("{base}/{cat}/index_{page}.html")
    };
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(&base), cfg.http.retry)?;
    Ok(parse_list(&html))
}

/// 详情。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site)?;
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        format!("{base}{id}")
    };
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA.to_string());
    let html = crate::http::get_text(&client, &url, &extra, cfg.http.retry)?;
    parse_detail(id, &html)
}

/// 搜索（帝国 CMS 表单搜索）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site)?;
    let url = format!("{base}/e/search/index.php");
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA.to_string());
    extra.insert("Origin".to_string(), base.clone());
    extra.insert("Referer".to_string(), format!("{base}/"));
    let mut form = HashMap::new();
    form.insert("show".to_string(), "title".to_string());
    form.insert("tempid".to_string(), "1".to_string());
    form.insert("tbname".to_string(), "article".to_string());
    form.insert("mid".to_string(), "1".to_string());
    form.insert("dopost".to_string(), "search".to_string());
    form.insert("submit".to_string(), String::new());
    form.insert("keyboard".to_string(), keyword.to_string());
    let html = crate::http::post_form_text(&client, &url, &extra, &form)?;
    Ok(parse_search(&html))
}

/// 播放信息：magnet 直出，不解析。
pub fn player_content(_cfg: &TvConfig, _site: &Site, _flag: &str, id: &str) -> Result<Value> {
    Ok(serde_json::json!({
        "parse": 0,
        "header": "",
        "playUrl": "",
        "url": id,
    }))
}

/* ============================================================
 * 解析
 * ============================================================ */

/// `#post_container .post_hover` → 视频卡片。
pub fn parse_list(html: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(list_row_sel()) {
        let Some(zoom) = row.select(zoom_sel()).next() else {
            continue;
        };
        let href = attr(&zoom, "href");
        if href.is_empty() {
            continue;
        }
        let name = attr(&zoom, "title");
        let cover = zoom
            .select(img_sel())
            .next()
            .map(|img| attr(&img, "src"))
            .filter(|s| !s.is_empty());
        let remark = rel_text(&row);
        out.push(VideoItem {
            id: href,
            name: if name.is_empty() {
                text_of(&zoom)
            } else {
                name
            },
            cover,
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

/// `#post_container [class=zoom]` → 搜索结果。
pub fn parse_search(html: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for zoom in doc.select(search_zoom_sel()) {
        let href = attr(&zoom, "href");
        if href.is_empty() {
            continue;
        }
        let raw_title = attr(&zoom, "title");
        let name = if raw_title.is_empty() {
            text_of(&zoom)
        } else {
            html_rule::strip_tags(&raw_title).trim().to_string()
        };
        let cover = zoom
            .select(img_sel())
            .next()
            .map(|img| attr(&img, "src"))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id: href,
            name,
            cover,
            remark: None,
        });
    }
    out
}

/// 详情解析。
pub fn parse_detail(id: &str, html: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);

    // 1) 播放线路：`#post_content` 下每个 table 一组 magnet。
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for content in doc.select(content_sel()) {
        for table in content.select(table_sel()) {
            let mut items: Vec<Episode> = Vec::new();
            for a in table.select(anchor_sel()) {
                let href = attr(&a, "href");
                if !href.starts_with("magnet") {
                    continue;
                }
                items.push(Episode {
                    title: text_of(&a),
                    url: href,
                });
            }
            if !items.is_empty() {
                groups.push(EpisodeGroup {
                    group: format!("magnet{}", groups.len() + 1),
                    items,
                });
            }
        }
    }

    // 2) 元数据：`.context` 各节点的 inner html 用 \n 拼接后跑中文标签正则。
    let context = doc
        .select(context_sel())
        .map(|el| el.inner_html())
        .collect::<Vec<_>>()
        .join("\n");

    let name = doc
        .select(h1_sel())
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();
    if name.is_empty() && groups.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            &Site::default(),
            "详情页缺少标题与播放线路（站点改版或非 SixV 模板？）",
        )
        .into_core());
    }

    let meta = Meta::from_context(&context);
    let cover = doc
        .select(content_img_sel())
        .next()
        .map(|img| attr(&img, "src"))
        .filter(|s| !s.is_empty());

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc: if meta.intro.is_empty() {
            None
        } else {
            Some(meta.intro)
        },
        year: opt(meta.year),
        area: opt(meta.area),
        director: opt(meta.director),
        actor: opt(meta.actor),
        episodes: groups,
    })
}

/// 详情页元数据（来自 `.context` 的中文标签块）。
#[derive(Debug, Default)]
pub struct Meta {
    pub type_name: String,
    pub year: String,
    pub area: String,
    pub remarks: String,
    pub actor: String,
    pub director: String,
    pub intro: String,
}

impl Meta {
    pub fn from_context(context: &str) -> Self {
        Self {
            type_name: capture(&type_re(), context),
            year: capture(&year_re(), context),
            area: capture(&area_re(), context),
            remarks: capture(&release_re(), context),
            actor: {
                let actor = capture(&actor_re(), context);
                if actor.is_empty() {
                    capture(&actor_alt_re(), context)
                } else {
                    actor
                }
            },
            director: clean_inline(&capture(&director_re(), context)),
            intro: clean_text(&capture(&intro_re(), context)),
        }
    }
}

/* ============================================================
 * 工具
 * ============================================================ */

/// 站点基址：`ext` 即 URL（去掉尾部 `/`）。
fn base_of(site: &Site) -> Result<String> {
    let ext = site.ext_string();
    let base = ext.trim().trim_end_matches('/').to_string();
    if base.is_empty() || !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "csp_SixV 需要站点 ext 提供基址（例如 https://www.6vdy.org）",
        )
        .into_core());
    }
    Ok(base)
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA.to_string());
    extra.insert("Referer".to_string(), format!("{base}/"));
    extra
}

fn attr(el: &scraper::ElementRef<'_>, name: &str) -> String {
    el.value()
        .attr(name)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn text_of(el: &scraper::ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

/// `[rel=category tag]` 的文本。CSS 属性值里含空格无法直接选择，
/// 这里退化为「所有带 rel 且值含 category 的元素」。
fn rel_text(row: &scraper::ElementRef<'_>) -> String {
    row.select(rel_sel())
        .find(|el| {
            el.value()
                .attr("rel")
                .map(|v| v.contains("category"))
                .unwrap_or(false)
        })
        .map(|el| text_of(&el))
        .unwrap_or_default()
}

fn opt(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn capture(re: &Regex, text: &str) -> String {
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_default()
}

/// 对应 Android 的 `N(...)` + `SN(...)`：去标签、去换行、实体替换。
fn clean_text(raw: &str) -> String {
    let mut out = html_rule::strip_tags(raw);
    out = out.replace('\n', "");
    out = out.replace("\u{3000}\u{3000}\u{3000}\u{3000}", "");
    out = out
        .replace("&amp;", "")
        .replace("middot;", "・")
        .replace("ldquo;", "【")
        .replace("rdquo;", "】");
    out.trim().to_string()
}

/// 对应 Android 的 `yq(...)`：保留 `<br>` 之外的换行语义，5 个全角空格变逗号。
fn clean_inline(raw: &str) -> String {
    let mut out = raw
        .replace("<br>", "")
        .replace("&nbsp;", "")
        .replace("&amp;", "")
        .replace("middot;", "・");
    out = out.replace("\u{3000}\u{3000}\u{3000}\u{3000}\u{3000}", ",");
    out.trim().to_string()
}

/// `◎类　　别　(.*?)<br>`
fn type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("◎类\u{3000}\u{3000}别\u{3000}(.*?)<br>").expect("类别正则"))
}

fn year_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("◎年\u{3000}\u{3000}代\u{3000}(.*?)<br>").expect("年代正则"))
}

fn area_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("◎产\u{3000}\u{3000}地\u{3000}(.*?)<br>").expect("产地正则"))
}

fn release_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("◎上映日期\u{3000}(.*?)<br>").expect("上映日期正则"))
}

fn actor_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // 原实现以 `</p>` 结尾；真实页面演员行通常止于 `<br>`，这里放宽到 `<br>` 以免把后续简介吞进来。
    RE.get_or_init(|| Regex::new("◎演\u{3000}\u{3000}员\u{3000}(.*?)<br>").expect("演员正则"))
}

fn actor_alt_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("◎主\u{3000}\u{3000}演\u{3000}(.*?)<br>").expect("主演正则"))
}

fn director_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("◎导\u{3000}\u{3000}演\u{3000}(.*?)<br>").expect("导演正则"))
}

fn intro_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("◎简\u{3000}\u{3000}介(.*?)<hr>").expect("简介正则"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_only_sixv_api() {
        assert!(handles("csp_SixV"));
        assert!(handles(" csp_sixv "));
        assert!(!handles("csp_Ysj"));
    }

    #[test]
    fn home_categories_are_the_fixed_twelve() {
        let cats = categories();
        assert_eq!(cats.len(), 12);
        assert_eq!(cats[0].type_id, "dianshiju/guoju");
        assert_eq!(cats[0].type_name, "国剧");
        assert_eq!(cats[11].type_id, "donghuapian");
    }

    #[test]
    fn parses_category_rows() {
        let html = r#"
        <div id="post_container">
          <div class="post_hover">
            <a class="zoom" href="/voddetail/1/" title="片一"><img src="/img/1.jpg"></a>
            <span rel="category tag">国剧</span>
          </div>
          <div class="post_hover">
            <a class="zoom" href="/voddetail/2/" title="片二"><img src="/img/2.jpg"></a>
            <span rel="category tag">日韩剧</span>
          </div>
        </div>"#;
        let items = parse_list(html);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "/voddetail/1/");
        assert_eq!(items[0].name, "片一");
        assert_eq!(items[0].cover.as_deref(), Some("/img/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("国剧"));
    }

    #[test]
    fn parses_detail_magnets_and_metadata() {
        let html = r#"
        <div class="article_container"><h1>片一 详情</h1></div>
        <div id="post_content">
          <img src="/img/1-big.jpg">
          <table><tr>
            <td><a href="magnet:?xt=urn:btih:aaa">第一集</a></td>
            <td><a href="magnet:?xt=urn:btih:bbb">第二集</a></td>
          </tr></table>
          <table><tr><td><a href="magnet:?xt=urn:btih:ccc">1080P</a></td></tr></table>
        </div>
        <div class="context"><p>
          ◎类　　别　国剧<br>◎年　　代　2024<br>◎产　　地　中国<br>
          ◎上映日期　2024-01-01<br>◎导　　演　张三<br>◎主　　演　李四<br>
          ◎简　　介<br>简介正文<hr>
        </p></div>"#;
        let detail = parse_detail("/voddetail/1/", html).expect("detail");
        assert_eq!(detail.name, "片一 详情");
        assert_eq!(detail.cover.as_deref(), Some("/img/1-big.jpg"));
        assert_eq!(detail.year.as_deref(), Some("2024"));
        assert_eq!(detail.area.as_deref(), Some("中国"));
        assert_eq!(detail.director.as_deref(), Some("张三"));
        assert_eq!(detail.actor.as_deref(), Some("李四"));
        assert!(detail.desc.as_deref().unwrap_or("").contains("简介正文"));
        assert_eq!(detail.episodes.len(), 2);
        assert_eq!(detail.episodes[0].group, "magnet1");
        assert_eq!(detail.episodes[0].items.len(), 2);
        assert_eq!(detail.episodes[0].items[1].url, "magnet:?xt=urn:btih:bbb");
        assert_eq!(detail.episodes[1].group, "magnet2");
    }

    #[test]
    fn parses_search_rows() {
        let html = r#"
        <div id="post_container">
          <a class="zoom" href="/voddetail/9/" title="片九"><img src="/img/9.jpg"></a>
        </div>"#;
        let items = parse_search(html);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "/voddetail/9/");
        assert_eq!(items[0].name, "片九");
    }

    #[test]
    fn detail_without_payload_is_an_error() {
        let err = parse_detail("/x/", "<html></html>").unwrap_err();
        assert!(err.to_string().contains("catvod.result.parse") || err.to_string().contains("详情页"));
    }
}
