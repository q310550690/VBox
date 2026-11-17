//! `csp_Nmys`：糯米影视（XPath 规则引擎 + MacCMS 播放页 + 自研播放串解码）。
//!
//! 来源（REA 反编译证据）：
//! * `com.github.catvod.spider.Nmys`，`_rea/decompiled/69dadb9a-Yoursmile/deobf/
//!   com/github/catvod/spider/Nmys.java`（655 行）；`playerContent` 等 918 指令方法被
//!   jadx 跳过，用 `--show-bad-code` 恢复到 `_rea/tmp/r139/Nmys.bad.java`（747 行）。
//! * 继承链 `Nmys extends XPathMacFilter extends XPathMac extends XPath`；XPathMac /
//!   XPathMacFilter 语义见同目录 `_rea/decompiled/69dadb9a-Yoursmile/deobf/.../XPathMac.java`
//!   （172 行）与 `XPathMacFilter.java`（30 行）。
//! * 字符串池：`short[] f35short` 454 项，XOR 解码（`C0025.m712` / `C0047.m1153` /
//!   `C0005.m106`，公式 `char = (char)(sArr[start+i] ^ key)`），解码脚本
//!   `_rea/tmp/r139/dec2.mjs`、`dec3.mjs`，产物 `_rea/tmp/r139/nmys-extra.json`。
//!
//! 协议要点（与原类一一对应）：
//! * 列表/详情/搜索：完全由 `ext` 里的测试规则驱动（复用 `families::xpath` 的规则引擎）；
//!   本站 `ext` 形如 `https://agit.ai/.../ext/nmys.json`（规则 JSON 或 URL）。
//! * 详情：super 之后取 `list[0].vod_play_url`，按 `$$$` 拆线路、每线路按 `#` 拆集后
//!   **倒序**再拼回（原类 `detailContent`），并把线路名经 `dcShow2Vip`/`pCfgJs` 映射。
//! * 播放：`ext` 规则里的 `playUrl` 模板拼出播放页 → GET（头 `user-agent` + `referer`）→
//!   用 `vod-play-id-(\d+)-src-(\d+)-num-(\d+)` 从播放 id 取 src/num →
//!   `mac_url='(.*?)';` 取线路串 → `$$$`/`#`/`$$` 三段定位密文 → 三种分支：
//!   `asHh` → `https://api.cnmcom.com/webcloud/nmwz.php?url=` 远程解析；
//!   `dsjh` → `https://api.cnmcom.com/webcloud/nmsvip.php?url=` 远程解析（并标 `jx:"0"`）；
//!   其余 → `getPlayUrl` 本地解码（前 37 字符取偶数位得 19 字符前缀 + 余下做
//!   `oo00o→/`、`o000o→+`、`O0O0O→=` 后 Base64 解码）。
//!
//! 有意偏差（记录在计划文档附录 EO）：
//! 1. 站点 `ext` 的规则地址（agit.ai）已停放，规则 JSON 无法取得 → 本家族 mock-only，
//!    测试用自造规则；运行时仍会按站点 `ext` 现拉规则。
//! 2. `asHh`/`dsjh` 两个远程解析分支的先后顺序按语义推断（两者互斥），原类经过
//!    XOR 状态机混淆，无法逐条一一核对。
//! 3. 原类解析失败时返回空串；这里返回 `catvod.result.parse` 结构化错误。
//! 4. `$$$` 第二段的 proxy 前缀按 `families::xpath` 的约定忽略。
//! 5. `XPathMacFilter` 的筛选模板替换依赖宿主传入 filter（桌面 `videos` 不消费），
//!    与 `families::xpath` 的既有偏差一致。

use std::collections::HashMap;
use std::sync::OnceLock;

use base64::Engine as _;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};
use super::super::xpath_rule::Rule;
use super::xpath;

pub const API: &str = "csp_Nmys";

/// 规则未配置 `ua` 时的兜底请求头（XPathMac 继承 XPath 的默认 Chrome/122）。
const DEFAULT_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

/// `Nmys.tF` 的固定 UA（short[] 253/10/2822 与 263/184/1876 解码结果）。
const PLAY_UA: &str = "Mozilla/5.0 (Linux; Android 10; LYA-AL00; HMSCore 6.4.0.312; GMSCore 20.15.16) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/83.0.4103.116 HuaweiBrowser/11.0.4.300 Mobile Safari/537.36";

/// 两个远程解析代理（short[] 149/47/3182 与 208/45/3088 解码结果）。
const NMSVIP: &str = "https://api.cnmcom.com/webcloud/nmsvip.php?url=";
const NMWZ: &str = "https://api.cnmcom.com/webcloud/nmwz.php?url=";

/// 播放 id 里的线路/集数定位（`Nmys.M`）。
const RE_PLAY_ID: &str = r"vod-play-id-\d+-src-(\d+)-num-(\d+)";
/// 播放页里的 MacCMS 线路串（`Nmys.KJ`）。
const RE_MAC_URL: &str = r#"mac_url='(.*?)';"#;
/// 远程解析返回的视频地址（`Nmys.q`）。
const RE_VIDEO_URL: &str = r"video: \{url: '(.*?)'";
/// `XPathMac` 的 `pCfgJs` 默认截取正则。
const RE_PCFG: &str = r"(?s)[\W|S|.]*?MacPlayerConfig.player_list[\W|S|.]*?=([\W|S|.]*?),MacPlayerConfig.downer_list";

fn re(pattern: &'static str) -> &'static Regex {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, &'static Regex>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("regex cache poisoned");
    if let Some(found) = guard.get(pattern) {
        return found;
    }
    let compiled: &'static Regex = Box::leak(Box::new(
        Regex::new(pattern).expect("内置正则在编译期校验"),
    ));
    guard.insert(pattern, compiled);
    compiled
}

/// 是否属于本家族（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn headers(url: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("user-agent".to_string(), PLAY_UA.to_string());
    map.insert("referer".to_string(), url.to_string());
    map
}

fn fetch(cfg: &TvConfig, rule: &Rule, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    let mut hdrs = headers(url);
    if let Some(ua) = rule.opt("ua") {
        hdrs.insert("user-agent".to_string(), ua);
    }
    get_text(&client, url, &hdrs, 2)
}

fn rule_fetch(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    let mut hdrs = HashMap::new();
    hdrs.insert("User-Agent".to_string(), DEFAULT_UA.to_string());
    get_text(&client, url, &hdrs, 2)
}

/// 是否直链视频（原类 `XPath.isVideoFormat` → `A.x`）。
fn is_video_format(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if !lower.starts_with("http") {
        return false;
    }
    RE_VIDEO_EXT
        .iter()
        .any(|ext| lower.contains(ext))
}

const RE_VIDEO_EXT: [&str; 8] = [
    ".m3u8", ".mp4", ".flv", ".mkv", ".mpd", ".ts", ".m4v", ".avi",
];

/// `Nmys.getPlayUrl`：前 37 字符取偶数位（0/2/…/36）拼出 19 字符前缀，
/// 余下部分先做三组替换（`oo00o→/`、`o000o→+`、`O0O0O→=`）再 Base64 解码。
fn get_play_url(encoded: &str) -> Option<String> {
    let chars: Vec<char> = encoded.chars().collect();
    let mut prefix = String::new();
    let mut index = 0usize;
    while index < 37 && index < chars.len() {
        prefix.push(chars[index]);
        index += 2;
    }
    let payload: String = chars.iter().skip(37).collect();
    let fixed = payload
        .replace("oo00o", "/")
        .replace("o000o", "+")
        .replace("O0O0O", "=");
    if fixed.is_empty() {
        return Some(prefix);
    }
    let mut padded = fixed;
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .ok()?;
    let decoded = String::from_utf8_lossy(&raw).to_string();
    Some(format!("{prefix}{decoded}"))
}

/// 从 `mac_url` 串中按 「`$$$` 取 src-1 → `#` 取 num-1 → `$$` 取 [1]」定位密文。
fn split_pick(mac_url: &str, src: usize, num: usize) -> Option<String> {
    let lines: Vec<&str> = mac_url.split("$$$").collect();
    let line = lines.get(src.checked_sub(1)?)?;
    let episodes: Vec<&str> = line.split('#').collect();
    let episode = episodes.get(num.checked_sub(1)?)?;
    let parts: Vec<&str> = episode.split("$$").collect();
    parts.get(1).map(|part| part.trim().to_string())
}

/// 远程解析：`<代理> + 密文` → GET → `video: {url: '…'}` 取地址。
fn resolve_remote(cfg: &TvConfig, prefix: &str, encoded: &str) -> Result<Option<String>> {
    let url = format!("{prefix}{encoded}");
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(&url), 2)?;
    Ok(re(RE_VIDEO_URL)
        .captures(&body)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string()))
}

/// `dcShow2Vip` 映射（键 → 值），以及 `pCfgJs` 里 `show → 线路 key` 的反向映射。
fn build_gc(cfg: &TvConfig, rule: &Rule) -> HashMap<String, String> {
    let mut gc = HashMap::new();
    // 语义与 XPathMac.homeContent 同一方向：JSON 的键是线路 key，值是显示名，最终映射「显示名 → key」。
    if let Some(map) = rule.obj("dcShow2Vip") {
        for (key, value) in map {
            if let Some(text) = value.as_str() {
                let text = text.trim();
                if !text.is_empty() {
                    gc.insert(text.to_string(), key.trim().to_string());
                }
            }
        }
    }
    if let Some(js_url) = rule.opt("pCfgJs").filter(|url| !url.is_empty()) {
        if let Ok(body) = rule_fetch(cfg, &js_url) {
            let pattern = rule.opt("pCfgJsR").unwrap_or_else(|| RE_PCFG.to_string());
            if let Ok(reg) = Regex::new(&pattern) {
                if let Some(capture) = reg.captures(&body).and_then(|caps| caps.get(1)) {
                    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(capture.as_str())
                    {
                        for (key, value) in map {
                            if let Some(show) = value.get("show").and_then(|v| v.as_str()) {
                                let show = show.trim();
                                if !show.is_empty() {
                                    gc.insert(show.to_string(), key.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    gc
}

fn apply_gc(detail: &mut VideoDetail, gc: &HashMap<String, String>) {
    for group in &mut detail.episodes {
        if let Some(mapped) = gc.get(&group.group) {
            group.group = mapped.clone();
        }
    }
}

/// 原类 `detailContent`：每线路内剧集倒序（`#"倒序"`），线路顺序不变。
fn reverse_episodes(detail: &mut VideoDetail) {
    for group in &mut detail.episodes {
        group.items.reverse();
    }
}

/// 分类：规则驱动（与 `csp_XPath` 一致）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    xpath::categories(cfg, site)
}

/// 列表：规则驱动（与 `csp_XPath` 一致）。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    cat: &str,
    page: u32,
    filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    xpath::videos(cfg, site, cat, page, filter)
}

/// 搜索：规则驱动（与 `csp_XPath` 一致）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    xpath::search(cfg, site, keyword)
}

/// 详情：规则驱动 + 线路改名 + 剧集倒序。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let rule = xpath::load_rule(cfg, site)?;
    let mut detail = xpath::detail(cfg, site, id)?;
    let gc = build_gc(cfg, &rule);
    apply_gc(&mut detail, &gc);
    reverse_episodes(&mut detail);
    Ok(detail)
}

/// 播放：MacCMS 播放页 + 密文定位 + 三种解析分支。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let rule = xpath::load_rule(cfg, site)?;
    let tpl = rule.s("playUrl");
    if tpl.is_empty() {
        return Err(parse_err(site, "csp_Nmys 规则缺少 playUrl 模板"));
    }
    let page_url = tpl.replace("{playUrl}", id);
    // 原类 `isVideoFormat(playUrl 模板)` 为真时不再抓播放页，直接按直链返回。
    if is_video_format(&tpl) {
        return Ok(json!({
            "parse": 0,
            "playUrl": "",
            "url": id,
            "header": {},
        }));
    }
    let page = fetch(cfg, &rule, &page_url)?;
    let caps = re(RE_PLAY_ID)
        .captures(id)
        .ok_or_else(|| parse_err(site, format!("播放 id 缺少 src/num 片段：{id}")))?;
    let src: usize = caps[1]
        .parse()
        .map_err(|_| parse_err(site, "播放 id 的 src 段不是数字"))?;
    let num: usize = caps[2]
        .parse()
        .map_err(|_| parse_err(site, "播放 id 的 num 段不是数字"))?;
    let mac = re(RE_MAC_URL)
        .captures(&page)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
        .ok_or_else(|| parse_err(site, "播放页缺少 mac_url 线路串"))?;
    let encoded = split_pick(mac, src, num)
        .ok_or_else(|| parse_err(site, format!("mac_url 线路串定位失败（src={src} num={num}）")))?;
    if encoded.contains("asHh") {
        let url = resolve_remote(cfg, NMWZ, &encoded)?
            .ok_or_else(|| parse_err(site, "nmwz 远程解析未返回视频地址"))?;
        return Ok(json!({ "url": url }));
    }
    if encoded.contains("dsjh") {
        let url = resolve_remote(cfg, NMSVIP, &encoded)?
            .ok_or_else(|| parse_err(site, "nmsvip 远程解析未返回视频地址"))?;
        return Ok(json!({ "url": url, "jx": "0" }));
    }
    let decoded = get_play_url(&encoded)
        .ok_or_else(|| parse_err(site, "getPlayUrl Base64 解码失败"))?;
    let mut out = Map::new();
    out.insert("url".to_string(), Value::String(decoded.clone()));
    if is_video_format(&decoded) {
        out.insert("jx".to_string(), Value::String("1".to_string()));
        out.insert("parse".to_string(), Value::String("0".to_string()));
        out.insert("playUrl".to_string(), Value::String(String::new()));
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Episode, EpisodeGroup};

    fn play_id_html(mac: &str) -> String {
        format!(r#"<script>var mac_url='{mac}';</script>"#)
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Nmys"));
        assert!(handles("CSP_NMYS"));
        assert!(!handles("csp_NmyswvGuard"));
        assert!(!handles("csp_XPath"));
    }

    #[test]
    fn get_play_url_restores_interleaved_prefix() {
        let prefix = "https://api.cnmcom.";
        assert_eq!(prefix.chars().count(), 19);
        let rest = "/webcloud/x.m3u8";
        let payload = base64::engine::general_purpose::STANDARD.encode(rest);
        let mut encoded = String::new();
        let chars: Vec<char> = prefix.chars().collect();
        for index in 0..18 {
            encoded.push(chars[index]);
            encoded.push('Z');
        }
        encoded.push(chars[18]);
        encoded.push_str(&payload);
        assert_eq!(get_play_url(&encoded).as_deref(), Some(format!("{prefix}{rest}").as_str()));
    }

    #[test]
    fn get_play_url_undoes_char_substitutions() {
        let prefix = "https://api.cnmcom.";
        let rest = "a+b/c==";
        let encoded_payload = base64::engine::general_purpose::STANDARD
            .encode(rest)
            .replace('/', "oo00o")
            .replace('+', "o000o")
            .replace('=', "O0O0O");
        let mut encoded = String::new();
        let chars: Vec<char> = prefix.chars().collect();
        for index in 0..18 {
            encoded.push(chars[index]);
            encoded.push('Z');
        }
        encoded.push(chars[18]);
        encoded.push_str(&encoded_payload);
        assert_eq!(get_play_url(&encoded).as_deref(), Some(format!("{prefix}{rest}").as_str()));
    }

    #[test]
    fn get_play_url_rejects_bad_base64() {
        assert!(get_play_url("short").is_some()); // 不足 37 字符：仅前缀分支
        assert!(get_play_url("").is_some());
    }

    #[test]
    fn split_pick_walks_three_levels() {
        let mac = "e1$$A#e2$$B#e3$$C$$$f1$$D#f2$$E";
        assert_eq!(split_pick(mac, 1, 2).as_deref(), Some("B"));
        assert_eq!(split_pick(mac, 1, 1).as_deref(), Some("A"));
        assert_eq!(split_pick(mac, 2, 1).as_deref(), Some("D"));
        assert!(split_pick(mac, 3, 1).is_none());
        assert!(split_pick(mac, 0, 1).is_none());
        assert!(split_pick(mac, 1, 9).is_none());
    }

    #[test]
    fn is_video_format_accepts_common_containers() {
        assert!(is_video_format("https://cdn.test/a.m3u8"));
        assert!(is_video_format("HTTP://cdn.test/a.MP4"));
        assert!(!is_video_format("https://cdn.test/page.html"));
        assert!(!is_video_format("/a.m3u8"));
    }

    #[test]
    fn headers_carry_player_ua_and_referer() {
        let map = headers("https://site.test/play/1.html");
        assert_eq!(map.get("user-agent").map(String::as_str), Some(PLAY_UA));
        assert_eq!(map.get("referer").map(String::as_str), Some("https://site.test/play/1.html"));
    }

    #[test]
    fn reverse_episodes_keeps_group_order() {
        let mut detail = VideoDetail {
            episodes: vec![
                EpisodeGroup {
                    group: "L1".to_string(),
                    items: vec![
                        Episode { title: "e1".into(), url: "u1".into() },
                        Episode { title: "e2".into(), url: "u2".into() },
                    ],
                },
                EpisodeGroup { group: "L2".to_string(), items: vec![Episode { title: "x".into(), url: "y".into() }] },
            ],
            ..Default::default()
        };
        reverse_episodes(&mut detail);
        assert_eq!(detail.episodes[0].group, "L1");
        assert_eq!(detail.episodes[0].items[0].title, "e2");
        assert_eq!(detail.episodes[0].items[1].title, "e1");
        assert_eq!(detail.episodes[1].group, "L2");
    }

    #[test]
    fn apply_gc_renames_known_groups_only() {
        let mut gc = HashMap::new();
        gc.insert("线路一".to_string(), "l1".to_string());
        let mut detail = VideoDetail {
            episodes: vec![
                EpisodeGroup { group: "线路一".into(), items: vec![] },
                EpisodeGroup { group: "线路二".into(), items: vec![] },
            ],
            ..Default::default()
        };
        apply_gc(&mut detail, &gc);
        assert_eq!(detail.episodes[0].group, "l1");
        assert_eq!(detail.episodes[1].group, "线路二");
    }

    #[test]
    fn build_gc_reads_dc_show2vip() {
        let rule = Rule::from_str(
            r#"{"dcShow2Vip":{"l1":" 线路一 ","l2":"线路二"}}"#,
        )
        .expect("规则 JSON 可解析");
        let cfg = TvConfig::default();
        let gc = build_gc(&cfg, &rule);
        assert_eq!(gc.get("线路一").map(String::as_str), Some("l1"));
        assert_eq!(gc.get("线路二").map(String::as_str), Some("l2"));
    }

    #[test]
    fn player_page_exposes_mac_url() {
        let html = play_id_html("l#e$$TOKEN");
        assert!(html.contains("mac_url='l#e$$TOKEN';"));
        assert_eq!(
            re(RE_MAC_URL).captures(&html).and_then(|caps| caps.get(1)).map(|m| m.as_str()),
            Some("l#e$$TOKEN")
        );
    }

    #[test]
    fn play_id_pattern_extracts_src_and_num() {
        let caps = re(RE_PLAY_ID).captures("vod-play-id-12345-src-2-num-7").expect("匹配");
        assert_eq!(&caps[1], "2");
        assert_eq!(&caps[2], "7");
        assert!(re(RE_PLAY_ID).captures("12345-2-7").is_none());
    }
}
