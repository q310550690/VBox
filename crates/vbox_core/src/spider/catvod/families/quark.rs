//! `csp_Quark`（夸克网盘分享 → 点播）原生实现。
//!
//! 来源：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Quark.java`
//! （211 行）与引擎 `parser/merge/A/Y.java`（1648 行，`N/R` 等方法）、
//! `parser/merge/I/a.java`（配置模型：`quark_cookie` 等）、`parser/merge/B/g.java`
//! 与 `parser/merge/B/j.java`（详情 JSON 模型）。
//!
//! 原实现要点：
//! * 原类**不覆写** `homeContent`/`categoryContent`/`searchContent`，首页分类、
//!   分类列表、搜索均为空；它是一个「分享 URL → 点播详情」的适配器
//!   （详情 id 就是夸克分享链接，通常由网盘搜索引擎或用户粘贴提供）。
//! * `init(ext)`：ext 按 `$$$` 切分，第一段要么是配置 JSON（`quark_cookie`、
//!   `vod_flags`、线程数等），要么是配置文件 URL（由客户端本地服务提供，如
//!   `http://127.0.0.1:9978/file/TV/quark.txt`）。
//! * `detailContent(list)`：取首个分享链接，正则
//!   `pan.quark.cn/s/([^\\?#/]+)(#*/list/share/([^\\?\\-]+))?` 取 share id 与
//!   子目录 fid，`UrlQuerySanitizer` 取 `pwd`；调引擎 `N(url, shareId, pass, fid)`
//!   获取分享树并组装 vod 详情（单线路）。
//! * `playerContent(id)`：id 以 `+` 分隔，形如 `{share_id}+{pass_code}+{file_id}`；
//!   调引擎 `R(...)` → 最终经代理拿到 `file/download` 直链，`parse:0`。
//!
//! 有意偏差（桌面端适配，不改变对外语义）：
//! 1. 原实现经 `Proxy.getReflectionUrl()+?do=quark&type=dwnz&...` 间接取直链；
//!    桌面端直接调用网盘引擎 `file/download`（等价结果），并保留 `file/v2/play`
//!    作为回退。
//! 2. 分享树按广度递归最多 3 层（原实现也是递归展开，但层数/上限未显式约束）；
//!    剧集标题用文件名，跨目录重名按 fid 去重。
//! 3. 配置读取兼容 `{"cookie":"..."}` 简化键，并支持 `drive_base` 覆盖接口根
//!    （仅测试/镜像使用；原引擎固定 `drive.quark.cn`）。
//! 4. 播放 id 的第三段之后若还有内容（原实现的 `+apk`/`+ebook` 等标记）忽略。
//! 5. 原实现播放前会做 `原畫`（高码率）分支与弹幕拼接；桌面端统一走下载直链，
//!    不做弹幕（宿主弹幕通道不同）。

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::quark::{QuarkClient, DRIVE_BASE, UA};
use crate::pan::{parse_share_link, PanConfig};

use super::super::contract::{CatvodError, ErrorClass};

/// 画像里的 api id。
pub const API: &str = "csp_Quark";
/// 详情里的线路名（原实现只生成单线路）。
pub const PLAY_GROUP: &str = "夸克网盘";
/// 播放 id 的分隔符（原实现 `str2.split("+")`）。
pub const PLAY_SEP: char = '+';
/// 分享树递归层数上限。
const MAX_DEPTH: usize = 3;
/// 单次详情最多收集的剧集数。
const MAX_EPISODES: usize = 500;

pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// 读取 ext 里的网盘配置。
///
/// 原类 `init` 先按 `$$$` 切分 ext，第一段既可以是配置 JSON，也可以是指向
/// 配置文件的 URL（TVBox 客户端本地服务，如
/// `http://127.0.0.1:9978/file/TV/quark.txt`），这里两种形态都支持。
pub fn load_config(cfg: &TvConfig, site: &Site) -> Result<PanConfig> {
    let ext = site.ext_string();
    let ext = ext.trim();
    if ext.is_empty() {
        return Ok(PanConfig::default());
    }
    let ext = ext.split("$$$").next().unwrap_or("").trim();
    if ext.starts_with("http://") || ext.starts_with("https://") {
        let client = crate::http::client_for(&cfg.http)?;
        let text = crate::http::get_text(&client, ext, &HashMap::new(), 1)?;
        return Ok(parse_config_text(&text));
    }
    Ok(parse_config_text(ext))
}

/// 从任意文本中提取配置 JSON（容忍前后杂字符；失败返回空配置）。
pub fn parse_config_text(text: &str) -> PanConfig {
    let text = text.trim();
    if let Ok(c) = serde_json::from_str::<PanConfig>(text) {
        return c;
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if start < end {
            if let Ok(c) = serde_json::from_str::<PanConfig>(&text[start..=end]) {
                return c;
            }
        }
    }
    PanConfig::default()
}

/// 构建 Quark 客户端（Cookie 与接口根都来自 ext 配置）。
pub fn make_client(cfg: &TvConfig, site: &Site) -> Result<(QuarkClient, PanConfig)> {
    let conf = load_config(cfg, site)?;
    let client = crate::http::client_for(&cfg.http)?;
    let base = conf.drive_base.trim();
    let base = if base.is_empty() {
        DRIVE_BASE.to_string()
    } else if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    Ok((
        QuarkClient::with_base(client, conf.effective_cookie(), &base),
        conf,
    ))
}

/// 播放 id：`{share_id}+{passcode}+{fid}`。
pub fn play_id(share_id: &str, passcode: &str, fid: &str) -> String {
    format!("{share_id}{PLAY_SEP}{passcode}{PLAY_SEP}{fid}")
}

/// 解析播放 id（第三段之后的内容忽略）。
pub fn parse_play_id(id: &str) -> Option<(String, String, String)> {
    let id = id.trim();
    let mut it = id.split(PLAY_SEP);
    let share_id = it.next()?.trim();
    let passcode = it.next()?.trim();
    let fid = it.next()?.trim();
    if share_id.is_empty() || fid.is_empty() {
        return None;
    }
    Some((share_id.to_string(), passcode.to_string(), fid.to_string()))
}

/// 在任意 JSON 里找一个 URL 字段（`file/v2/play` 响应结构随版本变化）。
pub fn find_url(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["url", "play_url", "download_url"] {
                if let Some(Value::String(s)) = map.get(key) {
                    if s.starts_with("http") {
                        return Some(s.clone());
                    }
                }
            }
            map.values().find_map(find_url)
        }
        Value::Array(items) => items.iter().find_map(find_url),
        _ => None,
    }
}

/// 递归收集分享目录内的视频文件（深度/数量有上限）。
#[allow(clippy::too_many_arguments)]
fn collect_videos(
    client: &QuarkClient,
    share_id: &str,
    stoken: &str,
    pdir_fid: &str,
    depth: usize,
    out: &mut Vec<(String, String)>,
    dirs: &mut Vec<String>,
    seen: &mut HashSet<String>,
) -> Result<()> {
    if depth > MAX_DEPTH || out.len() >= MAX_EPISODES {
        return Ok(());
    }
    let page = client.list_all(share_id, stoken, pdir_fid)?;
    for file in page.list {
        if out.len() >= MAX_EPISODES {
            break;
        }
        if file.is_dir() {
            if depth < MAX_DEPTH {
                if !dirs.iter().any(|d| d == &file.file_name) {
                    dirs.push(file.file_name.clone());
                }
                collect_videos(
                    client,
                    share_id,
                    stoken,
                    &file.fid,
                    depth + 1,
                    out,
                    dirs,
                    seen,
                )?;
            }
        } else if file.is_video() && seen.insert(file.fid.clone()) {
            out.push((file.file_name.clone(), file.fid));
        }
    }
    Ok(())
}

/// 详情：id 是夸克分享链接（可带 `?pwd=` 与 `#/list/share/{fid}`）。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let link = parse_share_link(id.trim())
        .ok_or_else(|| parse_err(site, "不是有效的夸克分享链接（pan.quark.cn/s/…）"))?;
    let (client, _conf) = make_client(cfg, site)?;
    let token = client.share_token(&link.share_id, &link.passcode)?;
    let root = if link.sub_fid.is_empty() {
        "0"
    } else {
        link.sub_fid.as_str()
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    collect_videos(
        &client,
        &link.share_id,
        &token.stoken,
        root,
        0,
        &mut out,
        &mut dirs,
        &mut seen,
    )?;

    let name = if token.title.trim().is_empty() {
        link.share_id.clone()
    } else {
        token.title.trim().to_string()
    };
    let items: Vec<Episode> = out
        .iter()
        .map(|(file_name, fid)| Episode {
            title: file_name.clone(),
            url: play_id(&link.share_id, &link.passcode, fid),
        })
        .collect();
    let desc = if dirs.is_empty() {
        None
    } else {
        Some(format!("目录：{}", dirs.join("、")))
    };
    let episodes = if items.is_empty() {
        Vec::new()
    } else {
        vec![EpisodeGroup {
            group: PLAY_GROUP.to_string(),
            items,
        }]
    };
    Ok(VideoDetail {
        id: id.trim().to_string(),
        name,
        desc,
        episodes,
        ..Default::default()
    })
}

/// 首页分类：原类未覆写 `homeContent`，返回空。
pub fn categories() -> Vec<Category> {
    Vec::new()
}

/// 分类列表：原类未覆写 `categoryContent`，返回空。
pub fn videos(
    _cfg: &TvConfig,
    _site: &Site,
    _cat: &str,
    _page: u32,
) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// 搜索：原类未覆写 `searchContent`，返回空。
pub fn search(_cfg: &TvConfig, _site: &Site, _keyword: &str) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// 播放：`{share_id}+{passcode}+{fid}` → 网盘直链（`parse:0`）。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let (_share_id, _passcode, fid) =
        parse_play_id(id).ok_or_else(|| parse_err(site, "播放 id 需为 {share_id}+{passcode}+{fid}"))?;
    let (client, _conf) = make_client(cfg, site)?;
    if !client.has_cookie() {
        return Err(CatvodError::new(
            ErrorClass::PanCookieRequired,
            site,
            "夸克直链需要 Cookie：请在站点 ext 中配置 {\"cookie\":\"...\"}",
        )
        .into_core());
    }
    let url = client.download_url(&fid).or_else(|first| {
        let info = client.play_info(&fid)?;
        find_url(&info).ok_or_else(|| {
            let class = ErrorClass::classify_pan_error(&first).unwrap_or(ErrorClass::Parse);
            CatvodError::new(class, site, format!("夸克直链解析失败：{first}")).into_core()
        })
    })?;
    Ok(json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

/* ============================================================
 * 测试
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        let mut s = Site::default();
        s.key = "quark".into();
        s.name = "夸克".into();
        s.api = API.into();
        s.ext = Some(json!(ext));
        s
    }

    #[test]
    fn handles_matches_case_insensitively() {
        assert!(handles("csp_Quark"));
        assert!(handles("CSP_QUARK"));
        assert!(!handles("csp_QuarkShare"));
        assert!(!handles("csp_Xb6v"));
    }

    #[test]
    fn config_parses_inline_quark_cookie() {
        let cfg = TvConfig::default();
        let conf = load_config(&cfg, &site(r#"{"quark_cookie":"ck=1","savepath":"/tv"}"#)).unwrap();
        assert_eq!(conf.effective_cookie(), "ck=1");
        assert_eq!(conf.savepath, "/tv");
    }

    #[test]
    fn config_splits_dollar_segments_and_keeps_generic_cookie() {
        let cfg = TvConfig::default();
        let conf = load_config(&cfg, &site(r#"{"cookie":"generic"}$$$thread_limit=8"#)).unwrap();
        assert_eq!(conf.effective_cookie(), "generic");
    }

    #[test]
    fn config_tolerates_noise_and_garbage() {
        let conf = parse_config_text(r#"var cfg = {"cookie":"c1"};"#);
        assert_eq!(conf.effective_cookie(), "c1");
        let empty = parse_config_text("not json at all");
        assert_eq!(empty.effective_cookie(), "");
    }

    #[test]
    fn empty_ext_yields_empty_config() {
        let cfg = TvConfig::default();
        let conf = load_config(&cfg, &site("")).unwrap();
        assert!(conf.effective_cookie().is_empty());
    }

    #[test]
    fn play_id_roundtrip() {
        let id = play_id("24f565ff8b77", "xy9z", "abc123");
        assert_eq!(id, "24f565ff8b77+xy9z+abc123");
        let (share, pass, fid) = parse_play_id(&id).unwrap();
        assert_eq!((share.as_str(), pass.as_str(), fid.as_str()), ("24f565ff8b77", "xy9z", "abc123"));
        // 第三段之后的标记忽略（原实现 `+apk` 等）
        let (share, _, fid) = parse_play_id("s1+p1+f1+apk").unwrap();
        assert_eq!((share.as_str(), fid.as_str()), ("s1", "f1"));
    }

    #[test]
    fn invalid_play_id_rejected() {
        assert!(parse_play_id("only-one-part").is_none());
        assert!(parse_play_id("s1+p1").is_none());
        assert!(parse_play_id("+p1+f1").is_none());
    }

    #[test]
    fn find_url_walks_nested_payload() {
        let v = json!({"status":200,"data":{"video":{"play_url":"https://cdn.test/x.m3u8"}}});
        assert_eq!(find_url(&v).as_deref(), Some("https://cdn.test/x.m3u8"));
        assert!(find_url(&json!({"data":{}})).is_none());
    }

    #[test]
    fn detail_rejects_invalid_share_link() {
        let cfg = TvConfig::default();
        let err = detail(&cfg, &site("{}"), "https://example.com/not-a-share").unwrap_err();
        assert!(err.to_string().contains("夸克分享链接"), "{err}");
    }

    #[test]
    fn player_requires_cookie() {
        let cfg = TvConfig::default();
        let err = player_content(&cfg, &site("{}"), "", "s1+p1+f1").unwrap_err();
        assert!(err.to_string().contains("Cookie"), "{err}");
    }

    #[test]
    fn player_rejects_malformed_id() {
        let cfg = TvConfig::default();
        let err = player_content(&cfg, &site("{}"), "", "bad-id").unwrap_err();
        assert!(err.to_string().contains("播放 id"), "{err}");
    }

    #[test]
    fn categories_and_lists_are_empty_like_original() {
        assert!(categories().is_empty());
        let cfg = TvConfig::default();
        assert!(videos(&cfg, &site("{}"), "1", 1).unwrap().is_empty());
        assert!(search(&cfg, &site("{}"), "kw").unwrap().is_empty());
    }
}
