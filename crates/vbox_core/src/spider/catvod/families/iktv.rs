//! `csp_Iktv` 原生实现（KTV 点歌：歌手 / 曲库 / 点歌，歌曲为 mkv 直链型）。
//!
//! 语义来自静态反编译：`_rea/decompiled/fc213119-jojo/deobf/com/github/catvod/spider/Iktv.java`
//! （275 行、0 处 `Method dump skipped`、无字符串混淆；另有 0f047745/dffec63f 变体同为
//! `extends Spider`）。类不依赖宿主私有 API，但 `ktv1` 分支使用 `showInput()`（App 输入框）
//! 取得关键词，桌面端无等价交互，故 `ktv1` 明确返回 `catvod.host.unsupported`。
//!
//! 原实现协议：
//! 1. `homeContent`：写死 3 个分类 `ktv3`=歌手、`ktv2`=曲库、`ktv1`=点歌。
//! 2. `categoryContent`：
//!    - `ktv2`：GET `{base}/searchmv?pg={page}`，响应为 JSON 数组，逐项取
//!      `number/name/singer_names` → id=`{number}$$${name}$$${singer_names}`、名称、固定封面、
//!      备注=`singer_names`；分页参数 (page, 50000, 20, 0)。
//!    - `ktv3`（其余分类同）：GET `{base}/searchmv?table=singer&pg={page}`，逐项 `id/name`
//!      → id=`{name}$$$singger`、名称、封面 `http://txysong.mysoto.cc/singers/{id}.jpg`。
//!    - `ktv1`：展示「歌手点歌 / 歌名点歌」两个虚拟入口，依赖 `showInput()` 输入关键词，
//!      桌面端不可达。
//! 3. `detailContent`：
//!    - 普通歌曲 id（`number$$$name$$$singer`）：单集「K歌专区」，
//!      `名称$http://txysong.mysoto.cc/songs/{number}.mkv`。
//!    - 歌手 id（`{name}$$$singger`）：GET
//!      `{base}/searchmv?where=singer_names&keywords={singer}&size=999` 取全部歌曲，
//!      单组「KTV」以 `#` 连接 `歌曲名$/songs/{number}.mkv`。
//! 4. `playerContent`：`url = id`（即 mkv 直链）、`header={User-Agent}`、`parse` 默认 0。
//! 5. `searchContent`：GET `{base}/searchmv?where=name&keywords={kw}&pg={page}`，解析同 `ktv2`。
//!
//! 与 Android 原实现的差异（均有意，注释内标注）：
//! - `ext` 首段为 http(s) 时覆盖接口基址（原实现写死；站点 ext 为 null 时行为一致）；
//! - 原代码循环 `for (i = 0; i < arr.length() - 1; i++)` 会漏掉最后一项，本实现完整遍历
//!   （保留全部结果，属明显 off-by-one 修正）；
//! - `ktv1` 的 `showInput()` 与 `AlertDialog` 属 App 交互，桌面端返回 `catvod.host.unsupported`；
//! - 搜索关键词用 `NON_ALPHANUMERIC` 百分号编码（Java 侧 `String.format` 未编码，此处更安全）；
//! - 详情字段中的提示文案（操作方法/音轨切换等）在原实现中由 obfuscated `h` 包装器写入多个
//!   `vod_*` 字段，本实现只保留名称/简介/线路，避免猜测字段映射。

use std::collections::HashMap;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 该家族处理的 api 名。
pub const API: &str = "csp_Iktv";

/// 原实现里的接口基址（`vpsdn.leuse.top`）。
const DEFAULT_BASE: &str = "https://vpsdn.leuse.top";

/// 歌曲文件基址（原实现写死 `txysong.mysoto.cc`）。
const SONG_BASE: &str = "http://txysong.mysoto.cc";

/// 原实现 UA（Chrome/117）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36";

/// 歌曲列表固定封面（原实现写死）。
const COVER_SONG: &str = "https://bpic.51yuansu.com/backgd/cover/00/19/71/5b93200cc5502.jpg?x-oss-process=image/resize,w_780/sharpen,100";

/// 点歌分类 id（原实现顺序 ktv3/ktv2/ktv1）。
const CATEGORIES: [(&str, &str); 3] = [("ktv3", "歌手"), ("ktv2", "曲库"), ("ktv1", "点歌")];

/// 是否处理该 api（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

/// 解析站点基址：`ext` 首段为 http(s) 时覆盖，默认 `https://vpsdn.leuse.top`。
pub fn base_of(site: &Site) -> Result<String> {
    let raw = site.ext_string();
    let first = raw.split("$$$").next().unwrap_or("").trim();
    if first.is_empty() {
        return Ok(DEFAULT_BASE.to_string());
    }
    if !(first.starts_with("http://") || first.starts_with("https://")) {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Iktv 的 ext 首段不是 http(s) 基址：{first}"),
        )
        .into_core());
    }
    Ok(first.trim_end_matches('/').to_string())
}

/// 固定 3 分类（原实现写死）。
pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

fn headers() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers
}

fn fetch_text(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(), 2)
}

/// 列表 URL：`ktv2` 取曲库，其余（`ktv3`/空）取歌手表。
fn list_url(base: &str, cat: &str, page: u32) -> String {
    if cat == "ktv2" {
        format!("{base}/searchmv?pg={page}")
    } else {
        format!("{base}/searchmv?table=singer&pg={page}")
    }
}

/// 搜索 URL。
fn search_url(base: &str, keyword: &str, page: u32) -> String {
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    format!("{base}/searchmv?where=name&keywords={kw}&pg={page}")
}

/// 歌手详情里的歌曲列表 URL（原实现 `size=999`、无 `pg`）。
fn singer_songs_url(base: &str, singer: &str) -> String {
    let kw = utf8_percent_encode(singer, NON_ALPHANUMERIC).to_string();
    format!("{base}/searchmv?where=singer_names&keywords={kw}&size=999")
}

/// 兼容「响应就是数组」与「响应是 `{"data": [...]}`」两种形态。
fn json_array(text: &str, site: &Site, what: &str) -> Result<Vec<Value>> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Iktv {what} 响应不是合法 JSON：{e}"),
        )
        .into_core()
    })?;
    match &value {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(_) => value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                CatvodError::new(
                    ErrorClass::Parse,
                    site,
                    format!("csp_Iktv {what} 响应缺少 data 数组"),
                )
                .into_core()
            }),
        _ => Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Iktv {what} 响应既不是数组也不是对象"),
        )
        .into_core()),
    }
}

fn str_of(item: &Value, key: &str) -> String {
    item.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 歌曲行 → VideoItem（id `number$$$name$$$singer_names`）。
fn songs_from_json(text: &str, site: &Site) -> Result<Vec<VideoItem>> {
    let items = json_array(text, site, "歌曲列表")?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let number = str_of(item, "number");
            if number.is_empty() {
                return None;
            }
            let name = str_of(item, "name");
            let singer = str_of(item, "singer_names");
            Some(VideoItem {
                id: format!("{number}$$${name}$$${singer}"),
                name,
                cover: Some(COVER_SONG.to_string()),
                remark: Some(singer),
            })
        })
        .collect())
}

/// 歌手行 → VideoItem（id `name$$$singger`）。
fn singers_from_json(text: &str, site: &Site) -> Result<Vec<VideoItem>> {
    let items = json_array(text, site, "歌手列表")?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let name = str_of(item, "name");
            if name.is_empty() {
                return None;
            }
            let id = str_of(item, "id");
            Some(VideoItem {
                id: format!("{name}$$$singger"),
                name,
                cover: Some(format!("{SONG_BASE}/singers/{id}.jpg")),
                remark: None,
            })
        })
        .collect())
}

/// 列表页（`ktv2` 曲库 / `ktv3` 歌手）。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    if cat == "ktv1" {
        return Err(CatvodError::new(
            ErrorClass::HostApi,
            site,
            "csp_Iktv 的 ktv1（点歌）依赖 App 输入框 showInput()，桌面端不支持；可用 ktv2 曲库 / ktv3 歌手或搜索",
        )
        .into_core());
    }
    let base = base_of(site)?;
    let url = list_url(&base, cat, page);
    let text = fetch_text(cfg, &url)?;
    let items = if cat == "ktv2" {
        songs_from_json(&text, site)?
    } else {
        singers_from_json(&text, site)?
    };
    Ok(items)
}

/// 搜索（`where=name`）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site)?;
    let url = search_url(&base, keyword, 1);
    let text = fetch_text(cfg, &url)?;
    songs_from_json(&text, site)
}

/// 单曲详情：单线路「K歌专区」。
fn song_detail(id: &str, parts: &[&str]) -> VideoDetail {
    let number = parts.first().copied().unwrap_or_default();
    let name = parts.get(1).copied().unwrap_or_default();
    let _singer = parts.get(2).copied().unwrap_or_default();
    let mut detail = VideoDetail::default();
    detail.id = id.to_string();
    detail.name = name.to_string();
    detail.cover = Some(COVER_SONG.to_string());
        detail.episodes = vec![EpisodeGroup {
        group: "K歌专区".to_string(),
        items: vec![Episode {
            title: name.to_string(),
            url: format!("{SONG_BASE}/songs/{number}.mkv"),
        }],
    }];
    detail
}

/// 歌手详情：把该歌手全部歌曲放进单组「KTV」。
fn singer_detail(id: &str, name: &str, songs: &[VideoItem]) -> VideoDetail {
    let mut urls = Vec::new();
    for song in songs {
        let parts: Vec<&str> = song.id.split("$$$").collect();
        let number = parts.first().copied().unwrap_or_default();
        if number.is_empty() {
            continue;
        }
        urls.push(Episode {
            title: song.name.clone(),
            url: format!("{SONG_BASE}/songs/{number}.mkv"),
        });
    }
    let mut detail = VideoDetail::default();
    detail.id = id.to_string();
    detail.name = name.to_string();
    detail.cover = Some(COVER_SONG.to_string());
        detail.episodes = vec![EpisodeGroup {
        group: "KTV".to_string(),
        items: urls,
    }];
    detail
}

/// 详情（按 id 末段区分歌曲 / 歌手）。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let parts: Vec<&str> = id.split("$$$").collect();
    if parts.len() >= 2 && parts[parts.len() - 1] == "singger" {
        let name = parts[0];
        let base = base_of(site)?;
        let url = singer_songs_url(&base, name);
        let text = fetch_text(cfg, &url)?;
        let songs = songs_from_json(&text, site)?;
        return Ok(singer_detail(id, name, &songs));
    }
    if parts.len() >= 2 {
        return Ok(song_detail(id, &parts));
    }
    Err(CatvodError::new(
        ErrorClass::Parse,
        site,
        format!("csp_Iktv 详情 id 格式非法：{id}"),
    )
    .into_core())
}

/// 播放：id 即 mkv 直链，原实现只带 UA、parse 默认 0。
pub fn player_content(_cfg: &TvConfig, _site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let url = id.trim();
    if url.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, _site, "csp_Iktv 播放地址为空").into_core());
    }
    Ok(json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Site;
    use serde_json::json;

    fn site(ext: Option<Value>) -> Site {
        let mut s = Site::default();
        s.key = "iktv".into();
        s.name = "KTV".into();
        s.api = API.into();
        s.ext = ext;
        s
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Iktv"));
        assert!(handles("CSP_IKTV"));
        assert!(!handles("csp_Xb6v"));
    }

    #[test]
    fn categories_are_three_fixed() {
        let cats = categories();
        assert_eq!(cats.len(), 3);
        assert_eq!(cats[0].type_id, "ktv3");
        assert_eq!(cats[1].type_id, "ktv2");
        assert_eq!(cats[2].type_id, "ktv1");
    }

    #[test]
    fn base_defaults_and_ext_override() {
        assert_eq!(base_of(&site(None)).unwrap(), DEFAULT_BASE);
        assert_eq!(base_of(&site(Some(json!("http://127.0.0.1:9/x/")))).unwrap(), "http://127.0.0.1:9/x");
        assert!(base_of(&site(Some(json!("not-a-url")))).is_err());
    }

    #[test]
    fn list_and_search_urls() {
        assert_eq!(list_url("http://b", "ktv2", 3), "http://b/searchmv?pg=3");
        assert_eq!(list_url("http://b", "ktv3", 2), "http://b/searchmv?table=singer&pg=2");
        assert_eq!(search_url("http://b", "海 阔", 1), "http://b/searchmv?where=name&keywords=%E6%B5%B7%20%E9%98%94&pg=1");
        assert_eq!(singer_songs_url("http://b", "周杰伦"), "http://b/searchmv?where=singer_names&keywords=%E5%91%A8%E6%9D%B0%E4%BC%A6&size=999");
    }

    #[test]
    fn songs_accept_raw_array_and_wrapped() {
        let raw = r#"[{"number":"123","name":"晴天","singer_names":"周杰伦"}]"#;
        let items = songs_from_json(raw, &site(None)).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "123$$$晴天$$$周杰伦");
        assert_eq!(items[0].remark.as_deref(), Some("周杰伦"));
        let wrapped = r#"{"data":[{"number":"9","name":"稻香","singer_names":"周杰伦"}]}"#;
        let items = songs_from_json(wrapped, &site(None)).unwrap();
        assert_eq!(items[0].id, "9$$$稻香$$$周杰伦");
    }

    #[test]
    fn singers_have_singger_id() {
        let raw = r#"[{"id":"55","name":"林俊杰"}]"#;
        let items = singers_from_json(raw, &site(None)).unwrap();
        assert_eq!(items[0].id, "林俊杰$$$singger");
        assert_eq!(items[0].cover, Some(format!("{SONG_BASE}/singers/55.jpg")));
    }

    #[test]
    fn song_detail_is_single_episode() {
        let d = song_detail("7$$$夜曲$$$周杰伦", &["7", "夜曲", "周杰伦"]);
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "K歌专区");
        assert_eq!(d.episodes[0].items[0].url, format!("{SONG_BASE}/songs/7.mkv"));
    }

    #[test]
    fn singer_detail_joins_all_songs() {
        let songs = vec![
            VideoItem { id: "1$$$A$$$X".into(), name: "A".into(), cover: None, remark: None },
            VideoItem { id: "2$$$B$$$X".into(), name: "B".into(), cover: None, remark: None },
        ];
        let d = singer_detail("X$$$singger", "X", &songs);
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "KTV");
        assert_eq!(d.episodes[0].items.len(), 2);
        assert!(d.episodes[0].items[0].url.ends_with("/songs/1.mkv"));
    }

    #[test]
    fn player_passes_direct_url() {
        let v = player_content(&TvConfig::default(), &site(None), "", "http://x/a.mkv").unwrap();
        assert_eq!(v["url"], "http://x/a.mkv");
        assert_eq!(v["parse"], 0);
    }

    #[test]
    fn ktv1_is_explicitly_unsupported() {
        let err = videos(&TvConfig::default(), &site(None), "ktv1", 1).unwrap_err();
        assert!(format!("{err:?}").contains("host"));
    }
}
