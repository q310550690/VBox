//! `csp_Push` 原生实现（推送型蜘蛛：把手机/PC 推来的播放地址变成一个线路）。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/ddaf13d7-0206/sources/com/github/catvod/spider/Push.java`
//!   （jadx 54 行、0 处跳过，未混淆；`_rea/decompiled/0f047745-FxzHm` 下同源变体结构一致）。
//! * 依赖的辅助类：`com.github.catvod.spider.merge.I.o`（`g()` 官方站判定、
//!   `f()` 直链正则判定）、`com.github.catvod.spider.merge.l.C0171b`（直链正则）、
//!   `com.github.catvod.spider.merge.b.f/h`（JSON 构造器）。
//!
//! 协议（原类逐段还原）：
//! * `class Push extends Spider`，无 home/category/search 覆写；`init` 只把 ext 交给
//!   `Ali.get().init`（阿里云盘 token 文件）。
//! * `detailContent(List)`：取 `list.get(0).trim()`：
//!   - 含 `aliyundrive` → 交给 `Ali.get().detailContent`（阿里云盘分享展开）；
//!   - 命中官方视频站（iqiyi/qq/youku/le/tudou/mgtv/sohu/acfun/bilibili/baofeng/pptv）
//!     → 线路名「官源」，`vod_play_url = "播放$" + url`，封面用固定图；
//!   - 命中直链正则（`http…(m3u8|mp4|flv|avi|mkv|rm|wmv|mpg|m4a)`）→ 线路名「直连」；
//!   - 其余 → 线路名「网页」。
//! * `playerContent(flag, id, list)`：
//!   - `flag` 含「画」→ 交 Ali 引擎（阿里云盘线路的「画质」前缀）；
//!   - `flag == "官源"` → `{"parse":1,"url":id}`；
//!   - `flag == "网页"` → `{"parse":1,"url":id}`；
//!   - 其余（「直连」）→ `{"parse":0,"url":id}`。
//!
//! 有意偏差（桌面端能力边界）：
//! 1. ext 是 TVBox 本地 token 文件路径（`127.0.0.1:9978` / `./lib/token.json`），
//!    桌面端没有该文件服务器：忽略 ext，阿里云盘走 `crate::pan::ali` 的匿名分享
//!    浏览（有 Cookie 时由 `pan` 配置提供）；
//! 2. 阿里云盘线路用本项目的 `pan` 引擎展开为「分享###fid」剧集，展开失败降级成
//!    单集「网页」（原类等价于把链接丢给客户端浏览器）；
//! 3. `flag` 含「画」的阿里云盘画质线路，本实现按网盘线路处理（含 `###` 时走引擎，
//!    否则按 `parse:1` 交给客户端）；
//! 4. 推送源没有分类/搜索，`videos` 返回 `catvod.result.parse` 错误、`search` 返回空；
//! 5. 站点不可达时无真站用例，本家族只用 mock/离线夹具验收。

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::AliClient;
use crate::pan::p115::P115Client;
use crate::pan::quark::QuarkClient;
use crate::pan::uc::UcClient;
use crate::pan::{parse_share_link, PanConfig, PanEntry, PanProvider, ShareLink};

use super::super::contract::{CatvodError, ErrorClass};

/// 单条分享展开的最大剧集数。
const MAX_EPISODES: usize = 500;

/// 站点 api 名。
pub const API: &str = "csp_Push";

/// 官方视频站域名（`o.g`）。
pub const OFFICIAL_SITES: [&str; 11] = [
    "iqiyi.com",
    "v.qq.com",
    "youku.com",
    "le.com",
    "tudou.com",
    "mgtv.com",
    "sohu.com",
    "acfun.cn",
    "bilibili.com",
    "baofeng.com",
    "pptv.com",
];

/// 固定封面（原类 `a()` 里写死的 bdstatic 图）。
pub const COVER: &str = "https://pic.rmb.bdstatic.com/bjh/1d0b02d0f57f0a42201f92caba5107ed.jpeg";

/// 原类 play_from 的三种取值。
pub const LINE_OFFICIAL: &str = "官源";
pub const LINE_DIRECT: &str = "直连";
pub const LINE_WEB: &str = "网页";
pub const LINE_ALI: &str = "阿里云盘";

/// 推送源没有分类浏览。
const NO_CATEGORY: &str = "推送源没有分类浏览（原类 expires Push，只有 detail/player）";

/// 是否本家族 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail.into()).into_core()
}

/// 是否官方视频站（`o.g`：11 个域名的 contains 判定）。
pub fn is_official_site(url: &str) -> bool {
    OFFICIAL_SITES.iter().any(|d| url.contains(d))
}

/// 是否直链（`C0171b.a` 的等价正则）。
pub fn is_direct_media(url: &str) -> bool {
    let u = url.trim();
    if !u.starts_with("http") {
        return false;
    }
    let head = u.split('?').next().unwrap_or(u);
    matches!(
        head.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref(),
        Some("m3u8" | "mp4" | "flv" | "avi" | "mkv" | "rm" | "wmv" | "mpg" | "m4a")
    )
}

/// 是否阿里云盘分享（原类只判 `aliyundrive`；桌面端同时识别 `alipan.com`）。
pub fn is_ali_share(url: &str) -> bool {
    url.contains("aliyundrive") || url.contains("alipan.com/s/")
}

/// 无分类。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Vec<Category> {
    Vec::new()
}

/// 无首页推荐。
pub fn home_videos(_cfg: &TvConfig, _site: &Site) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// 无分类浏览。
pub fn videos(_cfg: &TvConfig, site: &Site, _type_id: &str, _page: u32) -> Result<Vec<VideoItem>> {
    Err(parse_err(site, NO_CATEGORY))
}

/// 无搜索。
pub fn search(_cfg: &TvConfig, _site: &Site, _keyword: &str) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

fn load_pan_config(site: &Site) -> PanConfig {
    for seg in site.ext_string().split("$$$") {
        let seg = seg.trim();
        if seg.starts_with('{') {
            return PanConfig::parse(seg);
        }
    }
    PanConfig::default()
}

/// 构造 Ali 客户端（ali_base 非空时覆盖 API 基址：镜像/测试钩子）。
fn ali_client_for(cfg: &TvConfig, conf: &PanConfig) -> Result<AliClient> {
    let base = conf.ali_base.trim();
    if base.is_empty() {
        AliClient::with_rules(&cfg.http, conf.effective_ali_cookie())
    } else {
        let client = crate::http::client_for(&cfg.http)?;
        Ok(AliClient::with_base(
            client,
            conf.effective_ali_cookie(),
            base.trim_end_matches('/'),
        ))
    }
}

fn browse_share(cfg: &TvConfig, conf: &PanConfig, link: &ShareLink) -> Result<Vec<PanEntry>> {
    match link.provider {
        PanProvider::Quark => Ok(QuarkClient::with_rules(&cfg.http, conf.effective_cookie())?.browse(link)?.files),
        PanProvider::Uc => Ok(UcClient::with_rules(&cfg.http, conf.effective_uc_cookie())?.browse(link)?.files),
        PanProvider::Ali => Ok(ali_client_for(cfg, conf)?.browse(link)?.files),
        PanProvider::P115 => Ok(P115Client::with_rules(&cfg.http, conf.effective_p115_cookie())?.browse(link)?.files),
    }
}

fn is_video_name(name: &str) -> bool {
    matches!(
        name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref(),
        Some("mp4" | "mkv" | "m3u8" | "avi" | "ts" | "flv" | "wmv" | "mov" | "rmvb" | "webm" | "m4v" | "mpg" | "mpeg")
    )
}

fn find_url(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if s.starts_with("http") => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(find_url),
        Value::Object(m) => {
            for key in ["url", "play_url", "download_url", "raw_url"] {
                if let Some(Value::String(s)) = m.get(key) {
                    if s.starts_with("http") {
                        return Some(s.clone());
                    }
                }
            }
            m.values().find_map(find_url)
        }
        _ => None,
    }
}

fn resolve_play_url(
    cfg: &TvConfig,
    conf: &PanConfig,
    link: &ShareLink,
    fid: &str,
) -> Result<String> {
    match link.provider {
        PanProvider::Quark => {
            let c = QuarkClient::with_rules(&cfg.http, conf.effective_cookie())?;
            if let Ok(info) = c.play_info(fid) {
                if let Some(url) = find_url(&info) {
                    return Ok(url);
                }
            }
            c.download_url(fid)
        }
        PanProvider::Uc => {
            let c = UcClient::with_rules(&cfg.http, conf.effective_uc_cookie())?;
            if let Ok(info) = c.play_info(fid) {
                if let Some(url) = find_url(&info) {
                    return Ok(url);
                }
            }
            c.download_url(fid)
        }
        PanProvider::Ali => {
            let c = ali_client_for(cfg, conf)?;
            let token = c.share_token(&link.share_id, &link.passcode)?;
            c.download_url(&link.share_id, fid, &token.share_token)
        }
        PanProvider::P115 => {
            let c = P115Client::with_rules(&cfg.http, conf.effective_p115_cookie())?;
            c.download_url(&link.share_id, &link.passcode, fid)
        }
    }
}

/// 阿里云盘分享 → 阿里线路剧集（失败返回 None，由调用方降级「网页」）。
fn expand_ali(cfg: &TvConfig, conf: &PanConfig, url: &str) -> Option<Vec<Episode>> {
    let link = parse_share_link(url)?;
    if link.provider != PanProvider::Ali {
        return None;
    }
    let files = browse_share(cfg, conf, &link).ok()?;
    let mut episodes: Vec<Episode> = files
        .into_iter()
        .filter(|f| !f.is_dir && is_video_name(&f.name))
        .take(MAX_EPISODES)
        .map(|f| Episode {
            title: f.name.clone(),
            url: format!("{}###{}", link.url, f.fid),
        })
        .collect();
    if episodes.is_empty() {
        episodes = vec![Episode {
            title: "播放".to_string(),
            url: url.to_string(),
        }];
    }
    Some(episodes)
}

/// 详情：把推送来的地址映射为单线路。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "推送地址为空"));
    }
    let (line, episodes) = if is_ali_share(id) {
        let conf = load_pan_config(site);
        match expand_ali(cfg, &conf, id) {
            Some(eps) => (LINE_ALI, eps),
            None => (LINE_WEB, vec![Episode {
                title: "播放".to_string(),
                url: id.to_string(),
            }]),
        }
    } else {
        let line = if is_official_site(id) {
            LINE_OFFICIAL
        } else if is_direct_media(id) {
            LINE_DIRECT
        } else {
            LINE_WEB
        };
        (line, vec![Episode {
            title: "播放".to_string(),
            url: id.to_string(),
        }])
    };

    Ok(VideoDetail {
        id: id.to_string(),
        name: id.to_string(),
        cover: Some(COVER.to_string()),
        desc: None,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: line.to_string(),
            items: episodes,
        }],
    })
}

/// 播放：`###` 走网盘引擎；官源/网页 → parse 1；直连 → parse 0。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let flag = flag.trim();
    let id = id.trim();
    if id.contains("###") {
        let (share, fid) = id.rsplit_once("###").expect("contains ###");
        let Some(link) = parse_share_link(share) else {
            return Err(parse_err(site, format!("无法识别网盘分享：{share}")));
        };
        let conf = load_pan_config(site);
        let url = resolve_play_url(cfg, &conf, &link, fid)?;
        return Ok(json!({
            "parse": 0,
            "url": url,
            "jx": 0,
            "playUrl": "",
            "header": { "User-Agent": crate::spider::catvod::families::fourkhdr::UA },
        }));
    }
    if flag == LINE_OFFICIAL || flag == LINE_WEB || flag.contains('画') {
        return Ok(json!({ "parse": 1, "url": id }));
    }
    Ok(json!({ "parse": 0, "url": id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Option<&str>) -> Site {
        let mut s = Site {
            key: "push".to_string(),
            name: "推送".to_string(),
            api: API.to_string(),
            ..Default::default()
        };
        s.ext = ext.map(|e| json!(e));
        s
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Push"));
        assert!(handles(" CSP_PUSH "));
        assert!(!handles("csp_PushAgent"));
    }

    #[test]
    fn official_sites_are_recognised() {
        assert!(is_official_site("https://www.iqiyi.com/v_19r.html"));
        assert!(is_official_site("https://v.qq.com/x/cover/abc.html"));
        assert!(!is_official_site("https://example.org/a.m3u8"));
    }

    #[test]
    fn direct_media_forms_are_recognised() {
        assert!(is_direct_media("https://cdn.example.org/a/b.m3u8?token=1"));
        assert!(is_direct_media("http://cdn.example.org/x.mp4"));
        assert!(!is_direct_media("https://example.org/watch/1.html"));
        assert!(!is_direct_media("ftp://cdn.example.org/x.mp4"));
    }

    #[test]
    fn detail_official_uses_official_line() {
        let cfg = TvConfig::default();
        let d = detail(&cfg, &site(None), "https://www.iqiyi.com/v_19r.html").unwrap();
        assert_eq!(d.episodes[0].group, LINE_OFFICIAL);
        assert_eq!(d.episodes[0].items[0].title, "播放");
        assert!(d.episodes[0].items[0].url.contains("iqiyi.com"));
        assert_eq!(d.cover.as_deref(), Some(COVER));
    }

    #[test]
    fn detail_direct_uses_direct_line() {
        let cfg = TvConfig::default();
        let d = detail(&cfg, &site(None), " https://cdn.example.org/a.m3u8 ").unwrap();
        assert_eq!(d.episodes[0].group, LINE_DIRECT);
    }

    #[test]
    fn detail_other_uses_web_line() {
        let cfg = TvConfig::default();
        let d = detail(&cfg, &site(None), "https://example.org/watch/1.html").unwrap();
        assert_eq!(d.episodes[0].group, LINE_WEB);
    }

    #[test]
    fn detail_rejects_blank_id() {
        let cfg = TvConfig::default();
        let err = detail(&cfg, &site(None), "   ").unwrap_err();
        assert_parse_error(&err);
    }

    fn assert_parse_error(err: &CoreError) {
        let text = format!("{err}");
        assert!(text.contains("catvod.result.parse"), "unexpected error: {text}");
    }

    #[test]
    fn player_official_requires_parse_one() {
        let cfg = TvConfig::default();
        let v = player_content(&cfg, &site(None), LINE_OFFICIAL, "https://www.iqiyi.com/v_19r.html").unwrap();
        assert_eq!(v["parse"], json!(1));
    }

    #[test]
    fn player_direct_passes_through() {
        let cfg = TvConfig::default();
        let v = player_content(&cfg, &site(None), LINE_DIRECT, "https://cdn/a.m3u8").unwrap();
        assert_eq!(v["parse"], json!(0));
        assert_eq!(v["url"], json!("https://cdn/a.m3u8"));
    }

    #[test]
    fn player_rejects_unknown_share_on_triplet_id() {
        let cfg = TvConfig::default();
        let err = player_content(&cfg, &site(None), LINE_ALI, "not-a-share###fid").unwrap_err();
        assert_parse_error(&err);
    }

    #[test]
    fn categories_and_home_are_empty_videos_is_error() {
        let cfg = TvConfig::default();
        assert!(categories(&cfg, &site(None)).is_empty());
        assert!(home_videos(&cfg, &site(None)).unwrap().is_empty());
        assert!(search(&cfg, &site(None), "x").unwrap().is_empty());
        assert!(videos(&cfg, &site(None), "1", 1).is_err());
    }

    #[test]
    fn ali_share_forms_are_recognised() {
        assert!(is_ali_share("https://www.aliyundrive.com/s/abc"));
        assert!(is_ali_share("https://www.alipan.com/s/abc"));
        assert!(!is_ali_share("https://pan.quark.cn/s/abc"));
    }
}
