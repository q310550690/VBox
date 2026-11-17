//! `csp_Ysj` 原生实现（苹果 CMS / MacCMS v10 模板族，第三个垂直切片）。
//!
//! 语义来自静态反编译：`_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/Ysj.java`
//! （`dmmiku.com` 变体；`0f047745-FxzHm` 里是 `lldm.net` 变体，结构相同、UA 版本不同）。
//! 该类 `extends Spider`，无宿主私有 API 依赖，可 100% 原生重写：
//!
//! 1. 基址：类内写死 `https://www.dmmiku.com`；本站点 `ext` 为空，因此默认用内置基址，
//!    `ext` 若给出 http(s) 地址则覆盖（便于换站/本地 mock）。
//! 2. `homeContent`：`GET {base}/index.php/vod/show/id/20.html`；
//!    分类取 `div#hl02 a`（`href` 第 5 段是 URL 编码后的 class，如 `%E5%A5%87%E5%B9%BB`）；
//!    筛选组在 `div#hl03`(地区)/`#hl04`(年份)/`#hl05`(语言)/`#hl06`(字母)，VBox 模型无筛选位，忽略。
//! 3. `categoryContent(typeId, page)`：`{base}/index.php/vod/show` +
//!    [/class/{typeId}] + `/id/20` + [/page/{page}] + `.html`；
//!    行 = `li.vodlist_item` 内 `a.vodlist_thumb`（href / title / data-original），
//!    备注 = `span.pic_text`。
//! 4. `detailContent(id)`：`{base}/index.php/vod/detail/id/{id}`；
//!    标题 `h2.title`、封面 `a.vodlist_thumb[data-original]`、剧情 `div.content_desc > span`；
//!    元数据在 `li.data`：`年份：/地区：/类型：` 是同一条里的多个锚点，
//!    `状态：`、`主演：`、`导演：` 各占一条 —— 用标签前缀解析（比按索引更稳，见文末偏差说明）；
//!    播放列表 = `div.play_source_tab > a`（线路名）与 `ul.content_playlist`（按线路分组，
//!    每集 url 去掉 `/index.php/vod/play/id/` 前缀，`#` 分集、`$$$` 分线路）。
//! 5. `searchContent(word)`：`{base}/index.php/vod/search.html?wd={enc}&submit=`，
//!    行 = `li.searchlist_item`；命中「很抱歉，暂无相关视频」视为空结果。
//! 6. `playerContent`：返回播放页 URL（`{base}/index.php/vod/play/id/{id}`），`parse=1`
//!    表示需要后续嗅探/解析（原实现同样只给出播放页，不在此处解 m3u8）。

use std::collections::HashMap;
use std::sync::OnceLock;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

use crate::error::Result;
use crate::html_rule;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Ysj";

/// 类内写死的默认基址（`dmmiku.com` 变体）。
const DEFAULT_BASE: &str = "https://www.dmmiku.com";

/// 安卓端 UA（`Chrome/128`）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 选择器
 * ============================================================ */

macro_rules! selector {
    ($fn_name:ident, $css:expr) => {
        fn $fn_name() -> &'static Selector {
            static SEL: OnceLock<Selector> = OnceLock::new();
            SEL.get_or_init(|| Selector::parse($css).expect(concat!("选择器: ", $css)))
        }
    };
}

selector!(list_row_sel, "li.vodlist_item");
selector!(search_row_sel, "li.searchlist_item");
selector!(thumb_sel, "a.vodlist_thumb");
selector!(anchor_sel, "a");
selector!(pic_text_sel, "span.pic_text");
selector!(hl02_sel, "div#hl02");
selector!(title_sel, "h2.title");
selector!(data_sel, "li.data");
selector!(desc_sel, "div.content_desc > span");
selector!(source_tab_sel, "div.play_source_tab > a");
selector!(playlist_sel, "ul.content_playlist");

/* ============================================================
 * 入口
 * ============================================================ */

/// 站点基址：`ext` 覆盖 > 内置默认。
pub fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let trimmed = ext.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        DEFAULT_BASE.to_string()
    }
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

/// 首页分类：抓首页解析 `div#hl02 a`。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let url = format!("{base}/index.php/vod/show/id/20.html");
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    Ok(parse_categories(&html))
}

/// 分类列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let mut url = format!("{base}/index.php/vod/show");
    if !cat.trim().is_empty() {
        url.push_str("/class/");
        url.push_str(cat.trim());
    }
    url.push_str("/id/20");
    if page > 1 {
        url.push_str("/page/");
        url.push_str(&page.to_string());
    }
    url.push_str(".html");
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    Ok(parse_list(&html, &base))
}

/// 详情。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else if id.ends_with(".html") {
        format!("{base}/index.php/vod/detail/id/{id}")
    } else {
        format!("{base}/index.php/vod/detail/id/{id}.html")
    };
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    parse_detail(id, &base, &html)
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let enc = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/index.php/vod/search.html?wd={enc}&submit=");
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    Ok(parse_search(&html, &base))
}

/// 播放：返回播放页地址，交由后续嗅探/解析（`parse=1`）。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        format!("{base}/index.php/vod/play/id/{id}")
    };
    let _ = cfg;
    Ok(serde_json::json!({
        "parse": 1,
        "header": { "User-Agent": UA },
        "playUrl": "",
        "url": url,
    }))
}

/* ============================================================
 * 解析
 * ============================================================ */

/// 从详情/列表 href 取 id：MacCMS 链接形如
/// `/index.php/vod/detail/id/1618.html`，第 5 段（0 基）即 `1618.html`。
fn id_from_href(href: &str) -> String {
    let segs: Vec<&str> = href.split('/').collect();
    if let Some(seg) = segs.get(5) {
        if !seg.is_empty() {
            return (*seg).to_string();
        }
    }
    // 兜底：最后一段非空。
    segs.iter().rev().find(|s| !s.is_empty()).copied().unwrap_or("").to_string()
}

fn join_cover(base: &str, src: &str) -> String {
    if src.is_empty() {
        return String::new();
    }
    html_rule::join_url(base, src)
}

/// `div#hl02 a` → 分类（`href` 第 5 段为 URL 编码后的 class；「全部」链接含 `.html` 跳过）。
pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out: Vec<Category> = Vec::new();
    for hl02 in doc.select(hl02_sel()) {
        for a in hl02.select(anchor_sel()) {
            let href = attr(&a, "href");
            if href.is_empty() {
                continue;
            }
            let id = id_from_href(&href);
            if id.is_empty() || id.contains(".html") {
                continue;
            }
            let name = text_of(&a).replace("BD", "");
            let name = name.trim().to_string();
            if name.is_empty() || out.iter().any(|c| c.type_id == id) {
                continue;
            }
            out.push(Category {
                type_id: id,
                type_name: name,
            });
        }
    }
    out
}

/// `li.vodlist_item` → 列表。
pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    parse_rows(&Html::parse_document(html), list_row_sel(), base)
}

/// `li.searchlist_item` → 搜索结果。
pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    if html.contains("很抱歉，暂无相关视频") {
        return Vec::new();
    }
    parse_rows(&Html::parse_document(html), search_row_sel(), base)
}

fn parse_rows(doc: &Html, row_sel: &Selector, base: &str) -> Vec<VideoItem> {
    let mut out = Vec::new();
    for row in doc.select(row_sel) {
        let Some(a) = row.select(thumb_sel()).next().or_else(|| row.select(anchor_sel()).next()) else {
            continue;
        };
        let href = attr(&a, "href");
        if href.is_empty() {
            continue;
        }
        let id = id_from_href(&href);
        if id.is_empty() {
            continue;
        }
        let mut name = attr(&a, "title");
        if name.is_empty() {
            name = text_of(&a);
        }
        let cover = {
            let src = attr(&a, "data-original");
            if src.is_empty() { None } else { Some(join_cover(base, &src)) }
        };
        let remark = row
            .select(pic_text_sel())
            .next()
            .map(|el| text_of(&el))
            .map(|s| clean_text(&s))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id,
            name,
            cover,
            remark,
        });
    }
    out
}

/// 详情解析。
pub fn parse_detail(id: &str, base: &str, html: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);

    let name = doc
        .select(title_sel())
        .next()
        .map(|el| clean_text(&text_of(&el)))
        .unwrap_or_default();
    let cover = doc
        .select(thumb_sel())
        .next()
        .map(|a| attr(&a, "data-original"))
        .filter(|s| !s.is_empty())
        .map(|s| join_cover(base, &s));

    let mut year = None;
    let mut area = None;
    let mut actor = None;
    let mut director = None;

    for li in doc.select(data_sel()) {
        // 同一 `li.data` 里可能有多段「标签：值」（年份/地区/类型），
        // 用锚点 href 识别年份/地区，用文本前缀识别状态/主演/导演。
        for a in li.select(anchor_sel()) {
            let href = attr(&a, "href");
            let text = text_of(&a).trim().to_string();
            if text.is_empty() {
                continue;
            }
            if href.contains("/vod/search/year/") {
                year = Some(text);
            } else if href.contains("/vod/search/area/") {
                area = Some(text);
            } else if href.contains("/vod/search/actor/") && actor.is_none() {
                actor = Some(text);
            }
        }
        let flat = collapse(&text_of(&li));
        if let Some(v) = labeled(&flat, "主演") {
            actor = Some(v);
        } else if let Some(v) = labeled(&flat, "导演") {
            director = Some(v);
        }
    }

    let mut desc = doc
        .select(desc_sel())
        .next()
        .map(|el| text_of(&el))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    if desc.is_none() {
        if let Ok(sel) = Selector::parse("div.content_desc") {
            desc = doc
                .select(&sel)
                .next()
                .map(|el| text_of(&el))
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty());
        }
    }

    // 播放线路：`div.play_source_tab > a` 与 `ul.content_playlist` 一一对应。
    let mut episodes: Vec<EpisodeGroup> = Vec::new();
    let tabs: Vec<String> = doc.select(source_tab_sel()).map(|el| text_of(&el)).collect();
    for (idx, list) in doc.select(playlist_sel()).enumerate() {
        let mut items: Vec<Episode> = Vec::new();
        for a in list.select(anchor_sel()) {
            let href = attr(&a, "href");
            if href.is_empty() {
                continue;
            }
            let frag = href.replace("/index.php/vod/play/id/", "");
            items.push(Episode {
                title: text_of(&a).trim().to_string(),
                url: frag,
            });
        }
        if items.is_empty() {
            continue;
        }
        let group = tabs
            .get(idx)
            .map(|s| clean_text(s))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("线路{}", idx + 1));
        episodes.push(EpisodeGroup { group, items });
    }

    if name.is_empty() && episodes.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            &site_stub(id),
            "详情页缺少标题与播放列表",
        )
        .into_core());
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc,
        year,
        area,
        director,
        actor,
        episodes,
    })
}

/// `"状态：已完结 / 09-10"` → `Some("已完结")`。
fn labeled(flat: &str, label: &str) -> Option<String> {
    let pat = format!("{label}：");
    let idx = flat.find(&pat)?;
    let rest = &flat[idx + pat.len()..];
    let value = rest.split(['/', '\n']).next().unwrap_or("").trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 清理站点文案里的图标字体私用区字符（U+E000–U+F8FF）与不换行空格。
fn clean_text(s: &str) -> String {
    let filtered: String = s
        .chars()
        .filter(|c| !matches!(*c, '\u{E000}'..='\u{F8FF}') && *c != '\u{a0}')
        .collect();
    collapse(&filtered)
}

fn site_stub(key: &str) -> Site {
    Site {
        key: key.to_string(),
        ..Default::default()
    }
}

fn attr(el: &ElementRef, name: &str) -> String {
    el.value().attr(name).unwrap_or("").trim().to_string()
}

fn text_of(el: &ElementRef) -> String {
    el.text().collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_HTML: &str = r#"
<ul class="vodlist">
 <li class="vodlist_item">
  <a class="vodlist_thumb" href="/index.php/vod/detail/id/1618.html" title="暗杀教室 第二季" data-original="/upload/vod/a.jpg">
    <span class="pic_text text_right">已完结</span>
  </a>
 </li>
 <li class="vodlist_item">
  <a class="vodlist_thumb" href="/index.php/vod/detail/id/1617.html" title="暗杀教室" data-original="/upload/vod/b.jpg">
    <span class="pic_text text_right">更新至12集</span>
  </a>
 </li>
</ul>"#;

    const SEARCH_HTML: &str = r#"
<ul class="vodlist">
 <li class="searchlist_item">
  <a class="vodlist_thumb" href="/index.php/vod/detail/id/3451.html" title="身为暗杀者的我" data-original="/upload/vod/c.jpg"></a>
 </li>
</ul>"#;

    const DETAIL_HTML: &str = r#"
<h2 class="title">暗杀教室 第二季</h2>
<a class="vodlist_thumb" href="/index.php/vod/play/id/1618/sid/1/nid/1.html" title="暗杀教室 第二季" data-original="/upload/vod/a.jpg"></a>
<ul>
 <li class="data">
  <span>年份：</span><a href="/index.php/vod/search/year/2016.html">2016</a>
  <span>地区：</span><a href="/index.php/vod/search/area/%E6%97%A5%E6%9C%AC.html">日本</a>
  <span>类型：</span><a href="/index.php/vod/search/class/%E6%90%9E%E7%AC%91.html">搞笑</a>
 </li>
 <li class="data"><span>状态：</span><span class="data_style">已完结</span>&nbsp;/&nbsp;<em>09-10</em></li>
 <li class="data"><span>主演：</span>未知</li>
 <li class="data"><span>导演：</span><a href="/index.php/vod/search/director/x.html">ラルケ</a></li>
</ul>
<div class="content_desc context"><span><p>剧情简介文本</p></span></div>
<div class="play_source_tab"><a href="javascript:void(0);">主播放线路</a></div>
<div class="play_list_box"><ul class="content_playlist">
  <li><a href="/index.php/vod/play/id/1618/sid/1/nid/1.html">第1集</a></li>
  <li><a href="/index.php/vod/play/id/1618/sid/1/nid/2.html">第2集</a></li>
</ul></div>"#;

    const HOME_HTML: &str = r#"
<div id="hl02"><div class="scroller"><ul>
 <li><a href="/index.php/vod/show/id/20.html">全部</a></li>
 <li><a href="/index.php/vod/show/class/BD%E6%97%A0%E4%BF%AE/id/20.html">BD无修</a></li>
 <li><a href="/index.php/vod/show/class/%E5%A5%87%E5%B9%BB/id/20.html">奇幻</a></li>
</ul></div></div>"#;

    #[test]
    fn handles_api() {
        assert!(handles("csp_Ysj"));
        assert!(handles(" CSP_YSJ "));
        assert!(!handles("csp_YsjV2"));
    }

    #[test]
    fn parses_categories_skipping_all() {
        let cats = parse_categories(HOME_HTML);
        assert_eq!(cats.len(), 2, "实际: {cats:?}");
        assert_eq!(cats[0].type_id, "BD%E6%97%A0%E4%BF%AE");
        assert_eq!(cats[0].type_name, "无修");
        assert_eq!(cats[1].type_name, "奇幻");
    }

    #[test]
    fn parses_list_rows() {
        let items = parse_list(LIST_HTML, "https://www.dmmiku.com");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "1618.html");
        assert_eq!(items[0].name, "暗杀教室 第二季");
        assert_eq!(items[0].cover.as_deref(), Some("https://www.dmmiku.com/upload/vod/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("已完结"));
    }

    #[test]
    fn parses_search_rows() {
        let items = parse_search(SEARCH_HTML, "https://www.dmmiku.com");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "3451.html");
        assert_eq!(items[0].name, "身为暗杀者的我");
    }

    #[test]
    fn empty_search_message_yields_no_rows() {
        assert!(parse_search("很抱歉，暂无相关视频", "https://x").is_empty());
    }

    #[test]
    fn parses_detail_metadata_and_playlist() {
        let d = parse_detail("1618.html", "https://www.dmmiku.com", DETAIL_HTML).expect("详情");
        assert_eq!(d.name, "暗杀教室 第二季");
        assert_eq!(d.year.as_deref(), Some("2016"));
        assert_eq!(d.area.as_deref(), Some("日本"));
        assert_eq!(d.actor.as_deref(), Some("未知"));
        assert_eq!(d.director.as_deref(), Some("ラルケ"));
        assert_eq!(d.desc.as_deref(), Some("剧情简介文本"));
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "主播放线路");
        assert_eq!(d.episodes[0].items.len(), 2);
        assert_eq!(d.episodes[0].items[1].url, "1618/sid/1/nid/2.html");
    }

    #[test]
    fn cleans_icon_font_characters() {
        assert_eq!(clean_text("\u{e62f}\u{a0}主播放线路"), "主播放线路");
        assert_eq!(clean_text("线路 2"), "线路 2");
    }

    #[test]
    fn base_defaults_and_ext_override() {
        let mut site = Site::default();
        assert_eq!(base_of(&site), DEFAULT_BASE);
        site.ext = Some(serde_json::json!("http://127.0.0.1:9/"));
        assert_eq!(base_of(&site), "http://127.0.0.1:9");
    }
}
