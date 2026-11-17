//! `csp_Yinyuetai` 家族：音悦台（yinyuetai.com，音乐 MV 站）原生实现。
//!
//! 来源：`com.github.catvod.spider.Yinyuetai`（`dffec63f-pg.jar`，407 行，
//! `Method dump skipped` 0 处；`0f047745-FxzHm.jar` 361 行为等价变体，仅
//! 字符串常量以字节数组形式出现）。反编译目录：
//! `_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/Yinyuetai.java`。
//!
//! 协议（与原实现一一对应）：
//! - 首页固定 4 分类（channelId → 名称）：华语/欧美/韩语/日语。
//! - 分类列表：`GET {video}/video/explore/channelVideos?channelId={cid}&detailType=2&size=20&offset={(page-1)*20}`
//!   → `data[]{id,title,headImg,content}`；原实现把 pagecount 写死为 50000。
//! - 歌手锚点分页：分类 id 以 `$$${pg}` 结尾时（详情页里 artist 的 `[a=cr:{...}]` 锚点），
//!   只回一条「歌手」条目，id = `{artistId}$$${name}$$${cover}$$$act`。
//! - 详情（歌手页）：`GET {video}/video/listByArtist?artistId={id}&sinceId=0&size=999`
//!   → 每首 `{title}$fullClip.urls[0].url`，以 `#` 连接；线路名为原实现中的常量。
//! - 详情（MV）：`GET {video}/video/get?id={id}` → `data{title,headImg,content,artists[],fullClip.urls[]}`；
//!   线路 = 每条流的 `display`，行 = `url` 按 `#` 拆开；`content` 前加原常量前缀。
//! - 播放：`url = id`，header 仅 UA，`parse = 0`（直链 mp4/m3u8）。
//! - 搜索：`POST {search}/search/get_search_result.json`，body
//!   `{"searchType":"MV","key":"…","sinceId":0,"size":20,"requestTagRows":[]}`。
//!
//! 上游状态（2026-11-11 实测）与有意偏差：
//! 1. `*.yinyuetai.com` 证书于 **2026-10-30** 到期（DigiCert RapidSSL TLS RSA CA G1），
//!    严格 TLS 校验下全站 `CERT_HAS_EXPIRED`，而 DNS/HTTP 服务仍在线。为让该站可用，
//!    本家族**只对本站**使用 `http::client_for_insecure`（默认客户端仍严格校验；
//!    证书续期后应删除该调用）。
//! 2. `search-api.yinyuetai.com` DNS 已失效（ENOTFOUND）：搜索把底层网络错误原样
//!    上抛（原实现吞异常返回空串，桌面端改为明确报错）。
//! 3. `base_of` 允许 ext 首段 http(s) 覆盖视频/搜索基址（mock/镜像用；原实现写死域名）。
//! 4. `artists` 的 `[a=cr:{…}]name[/a]` 可点击标记在桌面端不渲染，落库为纯文本
//!    `名 A、名 B`（语义等价，展示更稳）。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::Result;
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 站点 api 名（与 TVBox 配置一致）。
pub const API: &str = "csp_Yinyuetai";

/// 视频 API 根（原实现写死）。
const VIDEO_BASE: &str = "https://video-api.yinyuetai.com";
/// 搜索 API 根（原实现写死；当前 DNS 已失效）。
const SEARCH_BASE: &str = "https://search-api.yinyuetai.com";
/// 原实现 UA（Chrome/128）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
/// 原实现里的「线路名」常量（歌手页/详情的固定前缀）。
const ARTIST_LINE: &str = "TG:https://t.me/TVBoxxoo @wexfnw 王二小放牛娃";
/// 歌手详情 id 后缀。
const ACT_SUFFIX: &str = "$$$act";
/// 歌手锚点分页后缀。
const PAGE_SUFFIX: &str = "$$${pg}";
/// 每页条数（原实现 size=20）。
const PAGE_SIZE: u32 = 20;
/// 首页固定分类：`(channelId, 名称)`。
const CATEGORIES: [(&str, &str); 4] = [
    ("6998499728862334976", "华语"),
    ("6951459197061984256", "欧美"),
    ("6998475633361805312", "韩语"),
    ("6997855138153095168", "日语"),
];

/// 是否为本站家族。
pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

/// 站点基址：默认写死两个域名；ext 为 http(s) 时统一覆盖（mock/镜像）。
fn bases(site: &Site) -> (String, String) {
    let ext = site.ext_string();
    let ext = ext.trim();
    if ext.starts_with("http://") || ext.starts_with("https://") {
        let base = ext.trim_end_matches('/').to_string();
        (base.clone(), base)
    } else {
        (VIDEO_BASE.to_string(), SEARCH_BASE.to_string())
    }
}

/// 请求头（原实现仅 UA）。
fn headers() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers
}

/// 建立客户端：本家族需要容忍上游已过期的证书（见模块头部偏差 1）。
fn client(cfg: &TvConfig) -> Result<reqwest::blocking::Client> {
    http::client_for_insecure(&cfg.http)
}

fn fetch(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = client(cfg)?;
    http::get_text(&client, url, &headers(), 2)
}

fn fetch_post_json(cfg: &TvConfig, url: &str, body: &str) -> Result<String> {
    let client = client(cfg)?;
    http::post_json_text(&client, url, &headers(), body)
}

fn parse_json(site: &Site, body: &str, what: &str) -> Result<Value> {
    serde_json::from_str(body.trim()).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Yinyuetai {what} JSON 解析失败: {e}"),
        )
        .into_core()
    })
}

/// 首页分类。
pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 分类列表 URL（原实现）。
fn channel_url(video_base: &str, channel_id: &str, page: u32) -> String {
    let offset = (page.max(1) - 1) * PAGE_SIZE;
    format!(
        "{video_base}/video/explore/channelVideos?channelId={channel_id}&detailType=2&size={PAGE_SIZE}&offset={offset}"
    )
}

/// 歌手列表 URL（原实现）。
fn artist_url(video_base: &str, artist_id: &str) -> String {
    format!("{video_base}/video/listByArtist?artistId={artist_id}&sinceId=0&size=999")
}

/// 详情 URL（原实现）。
fn video_url(video_base: &str, id: &str) -> String {
    format!("{video_base}/video/get?id={id}")
}

/// 搜索 URL（原实现）。
fn search_url(search_base: &str) -> String {
    format!("{search_base}/search/get_search_result.json")
}

/// 分类列表条目：`data[]{id,title,headImg,content}`。
fn parse_items(value: &Value) -> Vec<VideoItem> {
    let Some(data) = value.get("data").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let name = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let cover = item
                .get("headImg")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let remark = item
                .get("content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            Some(VideoItem {
                id,
                name,
                cover,
                remark,
            })
        })
        .collect()
}

/// 解析歌手锚点分类 id：`{artistId}$$${name}$$${cover}$$${pg}`。
fn parse_artist_anchor(cat: &str) -> Option<(&str, &str, &str)> {
    let anchor = cat.trim_end_matches(PAGE_SUFFIX);
    let mut parts = anchor.split("$$$");
    let artist_id = parts.next()?.trim();
    let name = parts.next()?.trim();
    let cover = parts.next()?.trim();
    if artist_id.is_empty() || name.is_empty() {
        return None;
    }
    Some((artist_id, name, cover))
}

/// 解析歌手详情 id：`{artistId}$$${name}$$${cover}$$$act`。
fn parse_artist_detail_id(id: &str) -> Option<(&str, &str, &str)> {
    let raw = id.trim_end_matches(ACT_SUFFIX);
    let mut parts = raw.split("$$$");
    let artist_id = parts.next()?.trim();
    let name = parts.next()?.trim();
    let cover = parts.next()?.trim();
    if artist_id.is_empty() || name.is_empty() {
        return None;
    }
    Some((artist_id, name, cover))
}

/// 分类条目：普通分类走 channelVideos；歌手锚点只回一条。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let (video_base, _) = bases(site);
    if cat.ends_with(PAGE_SUFFIX) {
        let Some((artist_id, name, cover)) = parse_artist_anchor(cat) else {
            return Err(CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("csp_Yinyuetai 歌手锚点无法解析: {cat}"),
            )
            .into_core());
        };
        let item_id = format!("{artist_id}$$${name}$$${cover}$$$act");
        return Ok(vec![VideoItem {
            id: item_id,
            name: name.to_string(),
            cover: if cover.is_empty() {
                None
            } else {
                Some(cover.to_string())
            },
            remark: Some("歌手".to_string()),
        }]);
    }
    let url = channel_url(&video_base, cat, page);
    let body = fetch(cfg, &url)?;
    let value = parse_json(site, &body, "channelVideos")?;
    let items = parse_items(&value);
    if items.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Yinyuetai 分类 {cat} 第 {page} 页为空"),
        )
        .into_core());
    }
    Ok(items)
}

/// 详情：歌手页（`$$$act`）与 MV 页两条分支。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    if id.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "csp_Yinyuetai 详情 id 为空").into_core());
    }
    let (video_base, _) = bases(site);
    if let Some((artist_id, name, cover)) = parse_artist_detail_id(id) {
        let url = artist_url(&video_base, artist_id);
        let body = fetch(cfg, &url)?;
        let value = parse_json(site, &body, "listByArtist")?;
        return parse_artist_detail(site, id, name, cover, &value);
    }
    let url = video_url(&video_base, id);
    let body = fetch(cfg, &url)?;
    let value = parse_json(site, &body, "video/get")?;
    parse_mv_detail(site, id, &value)
}

/// 从 `listByArtist` 的 JSON 中解析歌手页（全部作品归入固定线路）。
fn parse_artist_detail(site: &Site, id: &str, name: &str, cover: &str, value: &Value) -> Result<VideoDetail> {
    let items: Vec<Episode> = value
        .get("data")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let title = row.get("title")?.as_str()?.trim().to_string();
                    let url = row
                        .get("fullClip")?
                        .get("urls")?
                        .as_array()?
                        .first()?
                        .get("url")?
                        .as_str()?
                        .trim()
                        .to_string();
                    if title.is_empty() || url.is_empty() {
                        return None;
                    }
                    Some(Episode { title, url })
                })
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Yinyuetai 歌手页无作品: {id}"),
        )
        .into_core());
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name: name.to_string(),
        cover: if cover.is_empty() {
            None
        } else {
            Some(cover.to_string())
        },
        actor: Some(name.to_string()),
        episodes: vec![EpisodeGroup {
            group: ARTIST_LINE.to_string(),
            items,
        }],
        ..Default::default()
    })
}

/// 从 `video/get` 的 JSON 中解析 MV 详情（线路 = `display`，行 = `url` 按 `#` 拆开）。
fn parse_mv_detail(site: &Site, id: &str, value: &Value) -> Result<VideoDetail> {
    let data = value
        .get("data")
        .filter(|v| v.is_object())
        .ok_or_else(|| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("csp_Yinyuetai 详情 {id} 缺少 data"),
            )
            .into_core()
        })?;
    let name = data
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Yinyuetai 详情 {id} 缺少 title"),
        )
        .into_core());
    }
    let cover = data
        .get("headImg")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let content = data.get("content").and_then(|v| v.as_str()).unwrap_or_default();
    let artists: Vec<String> = data
        .get("artists")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("name").and_then(|v| v.as_str()))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut groups = Vec::new();
    if let Some(urls) = data
        .get("fullClip")
        .and_then(|v| v.get("urls"))
        .and_then(|v| v.as_array())
    {
        for row in urls {
            let display = row
                .get("display")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("默认");
            let raw = row.get("url").and_then(|v| v.as_str()).unwrap_or_default();
            let items: Vec<Episode> = raw
                .split('#')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .enumerate()
                .map(|(idx, item)| Episode {
                    title: if idx == 0 {
                        display.to_string()
                    } else {
                        format!("{display} {}", idx + 1)
                    },
                    url: item.to_string(),
                })
                .collect();
            if !items.is_empty() {
                groups.push(EpisodeGroup {
                    group: display.to_string(),
                    items,
                });
            }
        }
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc: Some(format!("{ARTIST_LINE}{content}")),
        actor: if artists.is_empty() {
            None
        } else {
            Some(artists.join("、"))
        },
        episodes: groups,
        ..Default::default()
    })
}

/// 搜索（原实现 POST JSON；上游搜索域名已失效，网络错误原样上抛）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let (_, search_base) = bases(site);
    let body = json!({
        "searchType": "MV",
        "key": keyword,
        "sinceId": 0,
        "size": PAGE_SIZE,
        "requestTagRows": [],
    })
    .to_string();
    let text = fetch_post_json(cfg, &search_url(&search_base), &body)?;
    let value = parse_json(site, &text, "search")?;
    let items = parse_items(&value);
    if items.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Yinyuetai 搜索「{keyword}」为空（上游 search-api 可能已下线）"),
        )
        .into_core());
    }
    Ok(items)
}

/// 播放：直链，原实现只设置 UA，parse 默认 0。
pub fn player_content(_cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let url = id.trim();
    if url.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "csp_Yinyuetai 播放地址为空").into_core());
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

    fn site(ext: Option<Value>) -> Site {
        let mut s = Site::default();
        s.key = "yinyuetai".to_string();
        s.name = "音悦台".to_string();
        s.api = API.to_string();
        s.ext = ext;
        s
    }

    #[test]
    fn handles_api_name() {
        assert!(handles("csp_Yinyuetai"));
        assert!(handles("CSP_YINYUETAI"));
        assert!(!handles("csp_Bili"));
    }

    #[test]
    fn categories_are_four_channels() {
        let cats = categories();
        assert_eq!(cats.len(), 4);
        assert_eq!(cats[0].type_name, "华语");
        assert_eq!(cats[0].type_id, "6998499728862334976");
        assert_eq!(cats[3].type_name, "日语");
    }

    #[test]
    fn bases_default_and_ext_override() {
        let (v, s) = bases(&site(None));
        assert_eq!(v, VIDEO_BASE);
        assert_eq!(s, SEARCH_BASE);
        let (v, s) = bases(&site(Some(json!("http://127.0.0.1:9/mock/"))));
        assert_eq!(v, "http://127.0.0.1:9/mock");
        assert_eq!(s, "http://127.0.0.1:9/mock");
    }

    #[test]
    fn channel_url_uses_page_offset() {
        let url = channel_url("https://video-api.yinyuetai.com", "6998499728862334976", 3);
        assert!(url.contains("channelId=6998499728862334976"));
        assert!(url.contains("size=20&offset=40"));
    }

    #[test]
    fn parse_items_reads_id_title_cover_remark() {
        let value = json!({
            "code": 0,
            "data": [
                {"id": "7075422331362983936", "title": "催眠大师 Live", "headImg": "https://img/x.jpg", "content": "吴青峰"},
                {"id": "", "title": "空id"}
            ]
        });
        let items = parse_items(&value);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "7075422331362983936");
        assert_eq!(items[0].name, "催眠大师 Live");
        assert_eq!(items[0].cover.as_deref(), Some("https://img/x.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("吴青峰"));
    }

    #[test]
    fn artist_anchor_round_trip() {
        let cat = "6926073053449290079$$$吴青峰$$$https://img/a.jpg$$${pg}";
        let (artist_id, name, cover) = parse_artist_anchor(cat).unwrap();
        assert_eq!(artist_id, "6926073053449290079");
        assert_eq!(name, "吴青峰");
        assert_eq!(cover, "https://img/a.jpg");
        let detail_id = format!("{artist_id}$$${name}$$${cover}$$$act");
        let (a, n, c) = parse_artist_detail_id(&detail_id).unwrap();
        assert_eq!((a, n, c), (artist_id, name, cover));
        assert!(parse_artist_detail_id("7075422331362983936").is_none());
    }

    #[test]
    fn videos_returns_artist_item_for_anchor() {
        let cat = "6926073053449290079$$$吴青峰$$$https://img/a.jpg$$${pg}";
        let items = videos(&TvConfig::empty(), &site(None), cat, 1).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "6926073053449290079$$$吴青峰$$$https://img/a.jpg$$$act");
        assert_eq!(items[0].name, "吴青峰");
        assert_eq!(items[0].remark.as_deref(), Some("歌手"));
    }

    #[test]
    fn mv_detail_groups_by_display() {
        let value = json!({
            "data": {
                "title": "催眠大师 Live",
                "headImg": "https://img/x.jpg",
                "content": "吴青峰 A 翻全场",
                "artists": [{"name": "吴青峰"}, {"name": "五月天"}],
                "fullClip": {"urls": [
                    {"display": "完整版2500码流", "url": "https://cdn/a-hd.mp4#https://cdn/a-2.mp4"},
                    {"display": "MP3播放地址", "url": "https://cdn/a.mp3"}
                ]}
            }
        });
        let mut s = site(None);
        s.key = "yinyuetai".into();
        let detail = parse_mv_detail(&s, "7075422331362983936", &value).unwrap();
        assert_eq!(detail.name, "催眠大师 Live");
        assert_eq!(detail.actor.as_deref(), Some("吴青峰、五月天"));
        assert!(detail.desc.unwrap().ends_with("吴青峰 A 翻全场"));
        assert_eq!(detail.episodes.len(), 2);
        assert_eq!(detail.episodes[0].group, "完整版2500码流");
        assert_eq!(detail.episodes[0].items.len(), 2);
        assert_eq!(detail.episodes[1].items[0].url, "https://cdn/a.mp3");
    }

    #[test]
    fn player_returns_direct_url() {
        let out = player_content(&TvConfig::empty(), &site(None), "线路", "https://cdn/a.mp4")
            .unwrap();
        assert_eq!(out["url"], "https://cdn/a.mp4");
        assert_eq!(out["parse"], 0);
        assert_eq!(out["header"]["User-Agent"], UA);
        assert!(player_content(&TvConfig::empty(), &site(None), "", "  ").is_err());
    }
}
