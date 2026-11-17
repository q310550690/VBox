//! `csp_Ying` 原生实现（樱花动漫 HTML 站点，2 站）。
//!
//! 语义来自静态反编译：`_rea/decompiled/5d982c72-_/deobf/com/github/catvod/spider/Ying.java`
//! （204 行，`extends Spider`，0 skips，字符串常量走 `prL.d()` XOR，已用
//! `_rea/tools/decode-prl.mjs` 全部解出）。类内基址**写死** `https://www.yhpdm.com`
//! （`0f047745-FxzHm` 版为 `https://www.iyhdmm.com`，两个版本协议一致），分类标签来自
//! 另一台写死的主机 `https://www.xmfans.me/yxsf/js/yx_catalog.js` 的 `_labels = [...]`
//! 内嵌 JSON。两处上游当前均不可达（DNS/TLS 失败）→ 仅 mock 验收（L-B）。
//!
//! 协议：
//! 1. 分类/筛选：`GET {catalog}/yx_catalog.js`，取 `_labels = ` 之后、`;` 之前的 JSON
//!    数组；形如 `[["genre","类型","全部","剧情",…],["region","地区","全部","大陆",…],…]`。
//!    原实现把 **region 行**的值当分类（`type_id` 即地区文本，列表用 `&region=` 过滤），
//!    其余行作为「筛选组」（key=tokens[0]、name=tokens[1]、value=tokens[2..]）。
//! 2. 列表 `GET {base}/list/?pagesize=24&pageindex={page-1}[&region={cat}]`；
//!    行 `div.lpic > ul > li`：`a[href]` 按 `/` 切第 3 段为 id（`/showp/{id}` 形态）、
//!    `h2` 文本为标题、`a > img[src]` 前补 `https:` 为封面、
//!    `span > font` 文本为备注（含 `:` 时取其后半段）。
//! 3. 详情 `GET {base}/showp/{id}`：标题 `div.rate > h1`、封面 `div.thumb > img[src]` 前补
//!    `https:`、简介 `div.info` 文本；`div.sinfo > span > a` 依次给出
//!    [0]=年份、[1]=地区、[2]=类型；线路名 `ul.menu0 > li` 与分集容器 `div.main0 > div`
//!    按下标配对，分集 `a` → `标题$href`（`#` 分集、`$$$` 分线路）。
//! 4. 搜索 `GET {base}/s_all?ex=1&kw={URLEncoder.encode(kw)}`，行同列表。
//! 5. 播放：`playerContent` 返回 `{parse:0, url: {base}{href}, header:{UA}}`（href 为
//!    详情里给出的相对播放页地址）。
//!
//! 相对原实现的有意偏差（桌面端语义差异，均已注释）：
//! - `ext` 首段为 http(s) 时覆盖基址、并派生 catalog 地址为 `{base}/yxsf/js/yx_catalog.js`
//!   （供 mock/自建镜像；默认仍写死两台真实主机）；
//! - 不再输出首页第一页视频（VBox 走分类+详情，不需要 `homeVideoContent`）；
//! - 列表筛选参数（`genre/letter/year/season/status/label/resource/order`）不接收
//!   —— VBox `Site` 无筛选承载位（与其它 HTML 家族一致），只保留 `region` 分类；
//! - 详情里 `年份/地区/类型` 改为按 `div.sinfo > span > a` 文本位置读取（与原实现一致），
//!   但缺项时返回空而不是空串拼接。

use std::collections::HashMap;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Ying";
/// 原实现写死的站点基址（`5d982c72` 版；`0f047745` 版为 `https://www.iyhdmm.com`）。
pub const DEFAULT_BASE: &str = "https://www.yhpdm.com";
/// 原实现写死的分类标签文件（另一台主机）。
pub const DEFAULT_CATALOG: &str = "https://www.xmfans.me/yxsf/js/yx_catalog.js";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36";
const PAGE_SIZE: u32 = 24;

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 基址与请求
 * ============================================================ */

/// 基址与 catalog 地址：`ext` 首段为 http(s) 时覆盖基址并派生 catalog（mock/镜像）。
fn endpoints(site: &Site) -> (String, String) {
    let ext = site.ext_string();
    let first = ext
        .split("$$$")
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/');
    if first.starts_with("http://") || first.starts_with("https://") {
        let base = first.to_string();
        let catalog = format!("{base}/yxsf/js/yx_catalog.js");
        return (base, catalog);
    }
    (DEFAULT_BASE.to_string(), DEFAULT_CATALOG.to_string())
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
 * 解析：catalog.js 标签
 * ============================================================ */

/// 从 `_labels = [...]` 里提取 JSON 数组（容忍 `;` 结尾与前后噪声）。
pub fn parse_labels(js: &str) -> Option<Vec<Vec<String>>> {
    let marker = "_labels = ";
    let start = js.find(marker)? + marker.len();
    let rest = &js[start..];
    let end = rest.find(';').unwrap_or(rest.len());
    let text = rest[..end].trim();
    let value: Value = serde_json::from_str(text).ok()?;
    let rows = value.as_array()?;
    let mut out = Vec::new();
    for row in rows {
        let Some(cells) = row.as_array() else { continue };
        let tokens: Vec<String> = cells
            .iter()
            .map(|cell| match cell {
                Value::String(text) => text.trim().trim_matches('"').to_string(),
                other => other.to_string(),
            })
            .filter(|token| !token.is_empty())
            .collect();
        if !tokens.is_empty() {
            out.push(tokens);
        }
    }
    Some(out)
}

/// 分类 = region 行的值（从第 3 个 token 起）；筛选组 = 其它行。
fn labels_to_categories(rows: &[Vec<String>]) -> Vec<Category> {
    let mut out = Vec::new();
    for row in rows {
        if !row.first().map(|key| key.eq_ignore_ascii_case("region")).unwrap_or(false) {
            continue;
        }
        for token in row.iter().skip(2) {
            if token == "全部" || token.is_empty() {
                continue;
            }
            out.push(Category {
                type_id: token.clone(),
                type_name: token.clone(),
            });
        }
    }
    out
}

/// 筛选结构：分类名 → 筛选组列表（与原实现「每个 region 都挂同一组 filters」一致）。
pub fn parse_filters(rows: &[Vec<String>]) -> Value {
    let mut groups: Vec<Value> = Vec::new();
    for row in rows {
        if row.first().map(|key| key.eq_ignore_ascii_case("region")).unwrap_or(false) {
            continue;
        }
        if row.len() < 3 {
            continue;
        }
        groups.push(json!({
            "key": row[0],
            "name": row[1],
            "value": row.iter().skip(2).cloned().collect::<Vec<String>>(),
        }));
    }
    if groups.is_empty() {
        return Value::Object(Default::default());
    }
    let classes = labels_to_categories(rows);
    let mut map = serde_json::Map::new();
    for category in classes {
        map.insert(category.type_name.clone(), Value::Array(groups.clone()));
    }
    Value::Object(map)
}

/* ============================================================
 * 解析：列表 / 详情
 * ============================================================ */

fn selector(text: &str) -> Selector {
    Selector::parse(text).expect("静态选择器")
}

fn join_cover(src: &str) -> Option<String> {
    let src = src.trim();
    if src.is_empty() {
        return None;
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        Some(src.to_string())
    } else if src.starts_with("//") {
        Some(format!("https:{src}"))
    } else {
        Some(format!("https:{src}"))
    }
}

fn remark_of(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // 原实现：备注含 `:` 时取分隔后的后半段（`状态 : 完结` → `完结`）。
    match text.rsplit_once(':') {
        Some((_, tail)) if !tail.trim().is_empty() => Some(tail.trim().to_string()),
        _ => Some(text.to_string()),
    }
}

pub fn parse_rows(html: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = selector("div.lpic > ul > li");
    let rows: Vec<_> = doc.select(&row_sel).collect();
    let mut out = Vec::new();
    for row in rows {
        let Some(anchor) = row.select(&selector("a")).next() else {
            continue;
        };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        // 原实现取 `href.split("/")[2]`（真实站点是相对路径 `/showp/{id}`）；
        // 这里改为「去查询串后取最后一段」，同时兼容绝对地址与相对地址。
        let path = href.split(['?', '#']).next().unwrap_or(href);
        let Some(id) = path.trim_end_matches('/').rsplit('/').next().filter(|id| !id.is_empty()) else {
            continue;
        };
        let name = row
            .select(&selector("h2"))
            .next()
            .map(|node| text_of(&node))
            .unwrap_or_default();
        let cover = row
            .select(&selector("a > img"))
            .next()
            .and_then(|img| img.value().attr("src"))
            .and_then(join_cover);
        let remark = row
            .select(&selector("span > font"))
            .next()
            .map(|node| text_of(&node))
            .and_then(|text| remark_of(&text));
        out.push(VideoItem {
            id: (*id).to_string(),
            name,
            cover,
            remark,
        });
    }
    out
}

fn text_of(node: &ElementRef) -> String {
    node.text().collect::<Vec<_>>().join("").trim().to_string()
}

pub fn parse_detail(site: &Site, id: &str, html: &str, base: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);
    let name = doc
        .select(&selector("div.rate > h1"))
        .next()
        .map(|node| text_of(&node))
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("详情页缺少标题（id={id}，站点结构可能已变）"),
            )
            .into_core()
        })?;

    let cover = doc
        .select(&selector("div.thumb > img"))
        .next()
        .and_then(|img| img.value().attr("src"))
        .and_then(join_cover);
    let desc = doc
        .select(&selector("div.info"))
        .next()
        .map(|node| text_of(&node))
        .filter(|text| !text.is_empty());

    // `div.sinfo > span > a`：[0]=年份、[1]=地区、[2]=类型
    let metas: Vec<String> = doc
        .select(&selector("div.sinfo > span > a"))
        .map(|node| text_of(&node))
        .collect();

    let group_names: Vec<String> = doc
        .select(&selector("ul.menu0 > li"))
        .map(|node| text_of(&node))
        .collect();
    let containers: Vec<_> = doc.select(&selector("div.main0 > div")).collect();

    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for (index, name) in group_names.iter().enumerate() {
        let Some(container) = containers.get(index) else {
            continue;
        };
        let mut items = Vec::new();
        for (episode_index, anchor) in container.select(&selector("a")).enumerate() {
            let href = anchor.value().attr("href").unwrap_or_default().trim();
            if href.is_empty() {
                continue;
            }
            let title = text_of(&anchor);
            items.push(Episode {
                title: if title.is_empty() {
                    format!("第{}集", episode_index + 1)
                } else {
                    title
                },
                url: href.to_string(),
            });
        }
        if !items.is_empty() {
            groups.push(EpisodeGroup {
                group: if name.trim().is_empty() {
                    format!("线路{}", index + 1)
                } else {
                    name.trim().to_string()
                },
                items,
            });
        }
    }

    let _ = base;
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc,
        year: metas.first().cloned().filter(|text| !text.is_empty()),
        area: metas.get(1).cloned().filter(|text| !text.is_empty()),
        director: None,
        actor: None,
        episodes: groups,
    })
}

/* ============================================================
 * 五入口
 * ============================================================ */

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let (_base, catalog) = endpoints(site);
    let js = fetch_text(cfg, &catalog)?;
    Ok(parse_labels(&js)
        .map(|rows| labels_to_categories(&rows))
        .unwrap_or_default())
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let (base, _catalog) = endpoints(site);
    let mut url = format!("{base}/list/?pagesize={PAGE_SIZE}&pageindex={}", page.saturating_sub(1));
    if !cat.trim().is_empty() && cat.trim() != "全部" {
        url.push_str("&region=");
        url.push_str(&utf8_percent_encode(cat.trim(), NON_ALPHANUMERIC).to_string());
    }
    let html = fetch_text(cfg, &url)?;
    Ok(parse_rows(&html))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let (base, _catalog) = endpoints(site);
    let url = format!("{base}/showp/{}", id.trim());
    let html = fetch_text(cfg, &url)?;
    parse_detail(site, id, &html, &base)
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let (base, _catalog) = endpoints(site);
    let encoded = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/s_all?ex=1&kw={encoded}");
    let html = fetch_text(cfg, &url)?;
    Ok(parse_rows(&html))
}

pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let (base, _catalog) = endpoints(site);
    let id = id.trim();
    if id.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "播放地址为空").into_core());
    }
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else if id.starts_with('/') {
        format!("{base}{id}")
    } else {
        format!("{base}/{id}")
    };
    let _ = cfg;
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": { "User-Agent": UA },
    }))
}

/* ============================================================
 * 单元测试
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site_stub() -> Site {
        Site {
            key: "ying".to_string(),
            name: "樱花".to_string(),
            api: API.to_string(),
            ..Default::default()
        }
    }

    const CATALOG: &str = r#"var _labels = [["genre","类型","全部","剧情","喜剧"],["region","地区","全部","大陆","香港"],["year","年份","全部","2024","2023"]];"#;

    #[test]
    fn handles_only_ying() {
        assert!(handles("csp_Ying"));
        assert!(handles(" CSP_YING "));
        assert!(!handles("csp_Yhpdm"));
    }

    #[test]
    fn endpoints_default_and_override() {
        let mut site = site_stub();
        let (base, catalog) = endpoints(&site);
        assert_eq!(base, DEFAULT_BASE);
        assert_eq!(catalog, DEFAULT_CATALOG);
        site.ext = Some(json!("http://127.0.0.1:8080"));
        let (base, catalog) = endpoints(&site);
        assert_eq!(base, "http://127.0.0.1:8080");
        assert_eq!(catalog, "http://127.0.0.1:8080/yxsf/js/yx_catalog.js");
    }

    #[test]
    fn parse_labels_and_categories() {
        let rows = parse_labels(CATALOG).expect("labels");
        assert_eq!(rows.len(), 3);
        let categories = labels_to_categories(&rows);
        assert_eq!(categories.len(), 2);
        assert_eq!(categories[0].type_id, "大陆");
        assert_eq!(categories[1].type_name, "香港");
        let filters = parse_filters(&rows);
        let groups = filters.get("大陆").and_then(|v| v.as_array()).expect("filters");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].get("key").and_then(|v| v.as_str()), Some("genre"));
    }

    #[test]
    fn parse_labels_rejects_garbage() {
        assert!(parse_labels("<html>no labels</html>").is_none());
    }

    #[test]
    fn parse_rows_maps_fields() {
        let html = r#"
        <div class="lpic"><ul>
          <li><a href="https://www.yhpdm.com/showp/123"><img src="//img.example/1.jpg"></a>
              <h2>示例番剧</h2><span><font>更新至12话</font></span></li>
          <li><a href="/showp/456"><img src="//img.example/2.jpg"></a>
              <h2>第二部</h2><span><font>状态 : 完结</font></span></li>
        </ul></div>"#;
        let rows = parse_rows(html);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "123");
        assert_eq!(rows[0].name, "示例番剧");
        assert_eq!(rows[0].cover.as_deref(), Some("https://img.example/1.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("更新至12话"));
        assert_eq!(rows[1].id, "456");
        assert_eq!(rows[1].remark.as_deref(), Some("完结"));
    }

    #[test]
    fn parse_detail_maps_meta_and_playlists() {
        let html = r#"
        <div class="rate"><h1>示例番剧</h1></div>
        <div class="thumb"><img src="//img.example/c.jpg"></div>
        <div class="info">一段简介</div>
        <div class="sinfo"><span><a>2024</a></span><span><a>大陆</a></span><span><a>动画</a></span></div>
        <ul class="menu0"><li>线路A</li><li>线路B</li></ul>
        <div class="main0">
          <div><a href="/p/1-1-1.html">第1集</a><a href="/p/1-1-2.html">第2集</a></div>
          <div><a href="/p/1-2-1.html">第1集</a></div>
        </div>"#;
        let detail = parse_detail(&site_stub(), "1", html, DEFAULT_BASE).expect("detail");
        assert_eq!(detail.name, "示例番剧");
        assert_eq!(detail.cover.as_deref(), Some("https://img.example/c.jpg"));
        assert_eq!(detail.desc.as_deref(), Some("一段简介"));
        assert_eq!(detail.year.as_deref(), Some("2024"));
        assert_eq!(detail.area.as_deref(), Some("大陆"));
        assert_eq!(detail.episodes.len(), 2);
        assert_eq!(detail.episodes[0].group, "线路A");
        assert_eq!(detail.episodes[0].items[1].url, "/p/1-1-2.html");
        assert_eq!(detail.episodes[1].group, "线路B");
    }

    #[test]
    fn parse_detail_missing_title_reports_parse_error() {
        let err = parse_detail(&site_stub(), "0", "<html></html>", DEFAULT_BASE).expect_err("fail");
        assert!(format!("{err:?}").contains("catvod.result.parse"), "{err:?}");
    }
}
