//! 豆瓣条目浏览（`csp_Douban`）原生实现（第 29 轮切片）。
//!
//! 来源：`_rea/decompiled/5fc50e17-5199d8f8bcbe3c6d6938ded33cb90596/deobf/com/github/catvod/spider/Douban.java`
//! （246 行、无 jadx 跳过；另有 222/313 行变体同协议）。
//!
//! 协议（`https://frodo.douban.com/api/v2`，`apikey=0ac44ae016490db2204ce0a042db2916`）：
//! * 请求头固定 `Host: frodo.douban.com`、`Referer: https://servicewechat.com/wx2f9b06c1de1ccfca/84/page-frame.html`、
//!   微信小程序 UA（Chrome/53 + MicroMessenger）——豆瓣对小程序来源返回 JSON，无 apikey 则 403。
//! * 首页 7 个分类：`hot_gaia` 热门电影、`tv_hot` 热播剧集、`show_hot` 热播综艺、`movie` 电影筛选、
//!   `tv` 电视筛选、`rank_list_movie` 电影榜单、`rank_list_tv` 电视剧榜单；首页首屏为
//!   `http://api.douban.com/api/v2/subject_collection/subject_real_time_hotest/items` 的 20 条。
//! * 分类列表按分类分支不同端点：
//!   - `hot_gaia`：`/movie/hot_gaia?sort={sort|recommend}&area={area|全部}&start={(page-1)*20}&count=20`，数组 `items`
//!   - `tv_hot` / `show_hot`：`/subject_collection/{type|tv_hot|show_hot}/items?start=&count=20`，数组 `subject_collection_items`
//!   - `movie` / `tv`：`/{movie|tv}/recommend?sort={sort|T}&tags={除 sort 外所有筛选项按逗号拼接（URL 编码）}&start=&count=20`，数组 `items`
//!   - `rank_list_movie` / `rank_list_tv`：`/subject_collection/{榜单|movie_real_time_hotest|tv_real_time_hotest}/items?...`
//! * 条目字段：`id`、`title`、`pic.normal`（封面追加 `@Referer=https://api.douban.com/@User-Agent=...` 供宿主图片代理带 Referer）、
//!   `rating.value` → 备注 `评分：{value}`；`id` 以 `msearch:` 前缀（Android 端约定：详情时跨站搜索该标题）。
//! * 原类没有 `detailContent`/`playerContent`/`searchContent` 覆写（豆瓣只有条目浏览，无播放资源），
//!   因此本实现把 detail/player 映射为明确 `catvod.host.unsupported`，search 返回空。
//!
//! 有意偏差：① `ext` 首段为 http(s) 时作为 API 镜像基址（便于 mock/镜像，原实现写死）；
//! ② `vod_remarks` 取评分值（原实现同）；③ 跳过无封面条目（原实现 `filterItemsWithoutPic` 行为等价）；
//! ④ `msearch:` 前缀保留在 id 中，详情报错信息里注明「需客户端跨站搜索」。

use std::collections::HashMap;

use serde_json::Value;

use crate::error::Result;
use crate::model::{Category, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// api 标识。
pub const API: &str = "csp_Douban";
/// 同协议 api：`DouDou` 是豆瓣克隆（3 站，第 131 轮并入本家族）。
pub const DOUDOU_API: &str = "csp_DouDou";
/// 默认 API 基址。
pub const DEFAULT_BASE: &str = "https://frodo.douban.com/api/v2";
/// 公开 apikey（原实现写死）。
pub const API_KEY: &str = "0ac44ae016490db2204ce0a042db2916";
/// 小程序 UA。
pub const UA: &str = "Mozilla/5.0 (Windows NT 6.1; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/53.0.2785.143 Safari/537.36 MicroMessenger/7.0.9.501 NetType/WIFI MiniProgramEnv/Windows WindowsWechat";
/// 小程序来源页。
pub const REFERER: &str = "https://servicewechat.com/wx2f9b06c1de1ccfca/84/page-frame.html";
/// 首页首屏集合。
pub const HOME_COLLECTION: &str = "subject_real_time_hotest";
/// 固定 7 分类：(type_id, type_name)。
pub const CATEGORIES: [(&str, &str); 7] = [
    ("hot_gaia", "热门电影"),
    ("tv_hot", "热播剧集"),
    ("show_hot", "热播综艺"),
    ("movie", "电影筛选"),
    ("tv", "电视筛选"),
    ("rank_list_movie", "电影榜单"),
    ("rank_list_tv", "电视剧榜单"),
];

/// 卡片封面追加的宿主图片代理标记（与 Android 原实现一致）。
const COVER_SUFFIX: &str = "@Referer=https://api.douban.com/@User-Agent=Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36";

/// 是否处理该 api（`csp_Douban` 与同协议克隆 `csp_DouDou`）。
pub fn handles(api: &str) -> bool {
    let api = api.trim();
    api.eq_ignore_ascii_case(API) || api.eq_ignore_ascii_case(DOUDOU_API)
}

/// 是否为豆瓣克隆 `csp_DouDou`（条目 id 形态不同）。
fn is_doudou(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(DOUDOU_API)
}

/// API 基址：`ext` 首段为 http(s) 时覆盖（mock/镜像）。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let first = ext.split("$$$").next().unwrap_or_default().trim().trim_end_matches('/');
    if first.starts_with("http://") || first.starts_with("https://") {
        return first.to_string();
    }
    DEFAULT_BASE.to_string()
}

fn headers() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("Host".to_string(), "frodo.douban.com".to_string());
    map.insert("Connection".to_string(), "Keep-Alive".to_string());
    map.insert("Referer".to_string(), REFERER.to_string());
    map.insert("User-Agent".to_string(), UA.to_string());
    map
}

fn category_content() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 分类列表（原实现无独立入口，走 categoryContent）。
pub fn categories() -> Vec<Category> {
    category_content()
}

fn get_json(cfg: &TvConfig, site: &Site, url: &str) -> Result<Value> {
    let client = crate::http::client_for(&cfg.http)?;
    let text = crate::http::get_text(&client, url, &headers(), 1).map_err(|err| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Douban 请求失败：{err}"),
        )
        .into_core()
    })?;
    serde_json::from_str::<Value>(&text).map_err(|err| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Douban 响应不是合法 JSON：{err}"),
        )
        .into_core()
    })
}

fn apikey_query() -> String {
    format!("apikey={API_KEY}")
}

fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn list_url(base: &str, cat: &str, page: u32, filter: &HashMap<String, String>) -> String {
    let page = page.max(1);
    let start = (page - 1) * 20;
    let pick = |key: &str, default: &str| -> String {
        filter
            .get(key)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() || key == "sort")
            .unwrap_or_else(|| default.to_string())
    };
    match cat {
        "hot_gaia" => format!(
            "{base}/movie/hot_gaia?{}&sort={}&area={}&start={start}&count=20",
            apikey_query(),
            encode(&pick("sort", "recommend")),
            encode(&pick("area", "全部"))
        ),
        "tv_hot" => format!(
            "{base}/subject_collection/{}/items?{}&start={start}&count=20",
            pick("type", "tv_hot"),
            apikey_query()
        ),
        "show_hot" => format!(
            "{base}/subject_collection/{}/items?{}&start={start}&count=20",
            pick("type", "show_hot"),
            apikey_query()
        ),
        "movie" | "tv" => {
            let sort = pick("sort", "T");
            let mut pairs: Vec<String> = filter
                .iter()
                .filter(|(k, _)| k.as_str() != "sort")
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            pairs.sort();
            let tags = encode(&pairs.join(","));
            format!(
                "{base}/{cat}/recommend?{}&sort={}&tags={tags}&start={start}&count=20",
                apikey_query(),
                encode(&sort)
            )
        }
        "rank_list_movie" => format!(
            "{base}/subject_collection/{}/items?{}&start={start}&count=20",
            pick("榜单", "movie_real_time_hotest"),
            apikey_query()
        ),
        "rank_list_tv" => format!(
            "{base}/subject_collection/{}/items?{}&start={start}&count=20",
            pick("榜单", "tv_real_time_hotest"),
            apikey_query()
        ),
        other => format!(
            "{base}/subject_collection/{}/items?{}&start={start}&count=20",
            encode(other),
            apikey_query()
        ),
    }
}

fn items(value: &Value) -> Vec<&Value> {
    for key in ["subject_collection_items", "items"] {
        if let Some(arr) = value.get(key).and_then(|v| v.as_array()) {
            return arr.iter().collect();
        }
    }
    Vec::new()
}

/// 把豆瓣条目转成 `VideoItem`（无封面条目跳过，与原实现 `filterItemsWithoutPic` 等价）。
pub fn parse_items_for(api: &str, value: &Value) -> Vec<VideoItem> {
    let doudou = is_doudou(api);
    let mut out = Vec::new();
    for item in items(value) {
        let id = item
            .get("id")
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        let name = item.get("title").and_then(|v| v.as_str()).unwrap_or_default().trim().to_string();
        if name.is_empty() || (!doudou && id.is_empty()) {
            continue;
        }
        let raw_pic = item
            .get("pic")
            .and_then(|p| p.get("normal"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(raw_pic) = raw_pic else {
            continue;
        };
        let cover = Some(raw_pic.clone() + COVER_SUFFIX);
        let remark = item
            .get("rating")
            .and_then(|r| r.get("value"))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .map(|v| format!("评分：{v}"));
        // 豆瓣克隆 DouDou 的条目 id 是 `msearch:###{title}###{pic}`（跨站搜索约定），原类如此。
        let item_id = if doudou {
            format!("msearch:###{name}###{raw_pic}")
        } else {
            format!("msearch:{id}")
        };
        out.push(VideoItem {
            id: item_id,
            name,
            cover,
            remark,
        });
    }
    out
}

/// 首页首屏（原实现 `homeContent` 的 list 部分）。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let url = format!(
        "http://api.douban.com/api/v2/subject_collection/{HOME_COLLECTION}/items?{}",
        apikey_query()
    );
    let value = get_json(cfg, site, &url)?;
    Ok(parse_items_for(&site.api, &value))
}

/// 分类列表。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    cat: &str,
    page: u32,
    filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let cat = if cat.trim().is_empty() { "hot_gaia" } else { cat.trim() };
    let url = list_url(&base, cat, page, filter);
    let value = get_json(cfg, site, &url)?;
    Ok(parse_items_for(&site.api, &value))
}

/// 豆瓣没有播放资源：详情返回明确的不支持错误（原类无 `detailContent`）。
pub fn detail(_cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    let hint = if id.starts_with("msearch:") {
        "豆瓣只提供条目浏览与评分，没有播放资源（原实现也没有 detailContent）；`msearch:` 条目需客户端按标题跨站搜索。"
    } else {
        "豆瓣只提供条目浏览与评分，没有播放资源（原实现也没有 detailContent）。"
    };
    Err(CatvodError::new(ErrorClass::HostApi, site, hint).into_core())
}

/// 豆瓣不提供播放资源（原类无 `playerContent`）。
pub fn player_content(_cfg: &TvConfig, site: &Site, _flag: &str, _id: &str) -> Result<Value> {
    Err(CatvodError::new(
        ErrorClass::HostApi,
        site,
        "豆瓣只提供条目浏览，没有播放地址（原实现也没有 playerContent）。",
    )
    .into_core())
}

/// 原类没有搜索覆写：返回空结果（与 Android 行为一致）。
pub fn search(_cfg: &TvConfig, _site: &Site, _keyword: &str) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// 豆瓣条目解析（`csp_Douban` 默认形态，测试与向后兼容用）。
#[allow(dead_code)]
pub fn parse_items(value: &Value) -> Vec<VideoItem> {
    parse_items_for(API, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_json() -> Value {
        serde_json::json!({
            "subject_collection_items": [
                {
                    "id": "36013664",
                    "title": "流浪地球3",
                    "pic": { "normal": "https://example.com/p1.jpg" },
                    "rating": { "value": 8.2 }
                },
                {
                    "id": "2",
                    "title": "无封面条目",
                    "pic": { "normal": "" }
                }
            ]
        })
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Douban"));
        assert!(handles("  CSP_douban "));
        assert!(handles("csp_DouDou"));
        assert!(handles(" CSP_doudou "));
        assert!(!handles("csp_Douban2"));
    }

    #[test]
    fn doudou_items_use_msearch_triplet_id() {
        let videos = parse_items_for(DOUDOU_API, &item_json());
        assert_eq!(videos.len(), 1, "实际: {videos:?}");
        assert_eq!(videos[0].id, "msearch:###流浪地球3###https://example.com/p1.jpg");
        assert_eq!(videos[0].name, "流浪地球3");
        assert!(videos[0].cover.as_deref().unwrap().starts_with("https://example.com/p1.jpg@Referer="));
        assert_eq!(videos[0].remark.as_deref(), Some("评分：8.2"));
    }

    #[test]
    fn categories_are_fixed_seven() {
        let cats = categories();
        assert_eq!(cats.len(), 7);
        assert_eq!(cats[0].type_id, "hot_gaia");
        assert_eq!(cats[6].type_name, "电视剧榜单");
    }

    #[test]
    fn parses_items_skipping_missing_cover() {
        let videos = parse_items(&item_json());
        assert_eq!(videos.len(), 1, "实际: {videos:?}");
        assert_eq!(videos[0].id, "msearch:36013664");
        assert_eq!(videos[0].name, "流浪地球3");
        assert!(videos[0].cover.as_deref().unwrap().starts_with("https://example.com/p1.jpg@Referer="));
        assert_eq!(videos[0].remark.as_deref(), Some("评分：8.2"));
    }

    #[test]
    fn hot_gaia_url_uses_filter_defaults() {
        let base = "https://frodo.douban.com/api/v2";
        let url = list_url(base, "hot_gaia", 2, &HashMap::new());
        assert!(url.starts_with("https://frodo.douban.com/api/v2/movie/hot_gaia?apikey="));
        assert!(url.contains("sort=recommend"));
        assert!(url.contains("start=20"));
    }

    #[test]
    fn movie_url_joins_filters_except_sort() {
        let base = "https://frodo.douban.com/api/v2";
        let mut filter = HashMap::new();
        filter.insert("类型".to_string(), "动作".to_string());
        filter.insert("地区".to_string(), "华语".to_string());
        filter.insert("sort".to_string(), "S".to_string());
        let url = list_url(base, "movie", 1, &filter);
        assert!(url.contains("sort=S"));
        assert!(url.contains("tags="));
        assert!(url.contains("%E5%8A%A8%E4%BD%9C%2C%E5%8D%8E%E8%AF%AD") || url.contains("%E5%8A%A8%E4%BD%9C") || url.contains("%2C"));
        assert!(url.contains("start=0"));
    }

    #[test]
    fn search_is_empty_and_detail_is_unsupported() {
        let cfg = TvConfig::default();
        let site = Site::default();
        assert!(search(&cfg, &site, "海贼").unwrap().is_empty());
        let err = detail(&cfg, &site, "msearch:1").unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("catvod.host.unsupported"), "实际: {msg}");
    }
}
