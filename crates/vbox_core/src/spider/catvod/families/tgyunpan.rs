//! `csp_TGYunPan` / `csp_TGYunPanLocal`（TG 频道搜索 → 网盘/磁力链接）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/tmp/w12/out1-deobf/com/github/catvod/spider/TGYunPan.java`
//!   （Vineflower，2000 行、0 skip）与
//!   `_rea/tmp/w12/lc-deobf/com/github/catvod/spider/TGYunPanLocal.java`（2298 行）。
//! * `_rea/tmp/w12/cfr-deobf/...` 交叉核对；`tgSearch` / `tgSearchChannel` 段 CFR
//!   反编译失败，行为以 Vineflower 输出为准。
//! * 可行性/协议分析：`_rea/tmp/w11w12-analysis.md` W12.2（:176-228）——两个 api 是
//!   同一个「tgsearch」伴生 HTTP 服务的客户端，逐行同构，仅默认基址与全局配置键名
//!   不同。
//!
//! 协议（逐段还原）：
//! * 请求：`POST {base}/`（`csp_TGYunPan`，`tgSearch`）或 `POST {base}/s/1`
//!   （`csp_TGYunPanLocal`，`tgSearchChannel`）；body 是
//!   `Base64(JSON{"page","encode","keyword","channelUsername"})`（`encode` 恒
//!   `"1"`，`page` 为字符串，字段顺序照上游 `JSONObject.put` 顺序），
//!   `Content-Type: application/json; charset=utf-8`；请求头只有 `User-Agent`
//!   与 `Referer`（= 基址）。
//! * 响应：按 `\n` 拆行 → 字典序排序 → 从末尾往前逐行解析。行内无空格时先按
//!   base64 解码（对应上游 `Base64.decode(line, 2)`）；消息本体按首个分隔符拆
//!   `chat`、`msgId`，剩余部分按制表符拆 `text` 与 `media`（无制表符时用空格
//!   分隔，且无 `media`）。
//! * 每条消息：`TGYunPanLocal.extractMovieDescription(text)` 取名/番号/封面；
//!   `media` 形如 `12345:I` / `12345:V` 时拼 `{base}/i|v/{msgId}/12345` 作图片/
//!   视频；正文去标签后用网盘链接正则与 magnet 正则提取分享链接，逐条去重后输出
//!   `msgId$$chat$$link$$name$$image$$video##`；`video` 非空时再追加一条把 video
//!   当链接的记录。
//! * 结果条目：magnet 逐参数处理（`&amp;` 还原、`&dn=` 名称前缀、前 60 字符截断）
//!   后作为 `vod_id`；`name` 为空时回退搜索词；按链接域名判类别（阿里/夸克/UC/
//!   115/迅雷/YD/PikPak/磁力/群组视频）并给固定封面；
//!   `remark = "{序号}.{类别}/{chat}/{msgId}"`。
//! * 分类：`homeContent` 固定一个 `JARSEARCH`（「搜索」）分类；`categoryContent`
//!   把 `type_id` 当关键词调搜索（`JARSEARCH` 首页是两个 Android UI 虚拟项）。
//! * 详情/播放：原类未覆写，继承 `Push`：磁力/直链直出，网盘分享交网盘引擎。
//!
//! 有意偏差（桌面端能力边界 / 上游现状）：
//! 1. ext：上游 `init` 只按 JSON 解析（`$$$` 文本样例的 `[1]` 基址不生效），桌面端
//!    同样只认 JSON（对象 ext 或 `$$$` 段里的 JSON 段）；无 JSON 时基址落
//!    `http://127.0.0.1:10199`。
//! 2. 上游 `w()` / `v()` 去尾斜杠的返回值未赋值（bug，实际保留尾斜杠、拼接出
//!    `//`）；桌面端规范化基址（去尾斜杠再拼路径）。
//! 3. `tgsearch_media_url` 上游读取后丢弃；桌面端忽略，媒体 URL 回退基址。
//! 4. 行 base64 解码失败时上游抛异常跳过整行；桌面端回退按明文解析。
//! 5. `extractMovieDescription` 的「番号」取 `A.group(2)`（上游行为，实际是 `:`/`：`
//!    分隔符）；照搬。其 F/G 兜底分支因 `e_name` 默认空格、非空判断恒真而不可达，
//!    未移植。
//! 6. 无全局 `panconfig`：`global_panconfig_tgsearch_url` / `..._local_url` /
//!    `..._channellist` / `..._media_url` 改由 ext JSON 的 `tgsearch_url` /
//!    `tgsearch_local_url` / `channellist` / `tgsearch_media_url` 注入。
//! 7. `categoryContent` 的 `JARSEARCHFOLDER`（Android 弹窗输入关键词）与
//!    `SUBSCRIBECONFIG` 虚拟项桌面端无对应交互，`videos` 返回空；`JARSEARCH`
//!    翻页保持上游「第二页起为空」。
//! 8. 详情/播放：上游继承 `Push`，网盘分享交网盘引擎解析；桌面端无引擎 → 详情
//!    单集直出 id，播放返回 `{parse:0, playUrl:"", url:id, header:{}}`（磁力/直链
//!    可播，网盘链接由客户端处理）。
//! 9. 图片代理 `b0.a.A(image, proxy)` 在开启代理时改写图片 URL；桌面端直接用原
//!    URL（未开代理时行为一致）。
//! 10. 空关键词桌面端不发请求直接返回空列表。
//! 11. 上游 `link.substring(0, 60)` 对短于 60 字符的 magnet 会抛异常；桌面端按
//!     实际长度截断。

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use regex::Regex;
use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::{client_for, post_json_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 本家族覆盖的 api（外部 tgsearch 服务，全站搜索）。
pub const API: &str = "csp_TGYunPan";

/// 本家族覆盖的 api（本地/频道模式，`/s/1` 端点）。
pub const API_LOCAL: &str = "csp_TGYunPanLocal";

/// 默认伴生服务基址（`TGYunPan.p` / `TGYunPanLocal.p`，两者仅差尾斜杠）。
const DEFAULT_BASE: &str = "http://127.0.0.1:10199";

/// 详情线路名（原类继承 `Push`，桌面端固定）。
const GROUP: &str = "TG 搜索";

/// `homeContent` 固定的「搜索」分类 id。
const CATEGORY_ALL: &str = "JARSEARCH";

/// `homeContent` 固定的「搜索」分类名。
const CATEGORY_ALL_NAME: &str = "搜索";

/// Android 弹窗输入关键词的虚拟分类 id（桌面端不支持）。
const CATEGORY_FOLDER: &str = "JARSEARCHFOLDER";

/// 搜索请求头 UA（`TGYunPan.getHeaders()`）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// magnet `vod_id` 截断长度（上游 `substring(0, 60)`）。
const MAGNET_ID_LEN: usize = 60;

/// 类别固定封面：阿里。
const COVER_ALI: &str = "https://inews.gtimg.com/newsapp_bt/0/13263837859/1000";
/// 类别固定封面：夸克。
const COVER_QUARK: &str =
    "https://bkimg.cdn.bcebos.com/pic/6d81800a19d8bc3e93bd4e218c8ba61ea9d345a7";
/// 类别固定封面：115。
const COVER_115: &str = "https://bkimg.cdn.bcebos.com/pic/f2deb48f8c5494eeb95e781a24f5e0fe99257eb0";
/// 类别固定封面：UC。
const COVER_UC: &str = "https://bkimg.cdn.bcebos.com/pic/6a600c338744ebf85926ac2bd2f9d72a6059a74b";
/// 类别固定封面：迅雷。
const COVER_THUNDER: &str =
    "https://bkimg.cdn.bcebos.com/pic/d4628535e5dde7114164dfafa8efce1b9d166131";
/// 类别固定封面：PikPak。
const COVER_PIKPAK: &str =
    "https://i0.hdslb.com/bfs/archive/5ae3b02b3c3246afda72ac84a497475dc29187c7.jpg";
/// 类别固定封面：磁力。
const COVER_MAGNET: &str =
    "https://img-blog.csdnimg.cn/img_convert/09f13ae9df32c90920a200f82e7538b0.png";

/// 是否本家族覆盖的 api（`csp_TGYunPan` / `csp_TGYunPanLocal`）。
pub fn handles(api: &str) -> bool {
    let api = api.trim();
    api.eq_ignore_ascii_case(API) || api.eq_ignore_ascii_case(API_LOCAL)
}

/// 是否为本地/频道模式（决定端点与 ext 键优先级）。
fn is_local(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API_LOCAL)
}

/// 统一构造 `catvod.result.parse` 错误。
fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// 统一构造 `catvod.net.http` 错误。
fn net_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Network, site, detail).into_core()
}

/// 把 ext 按 `$$$` 拆成段（去首尾空白）。
fn ext_segments(site: &Site) -> Vec<String> {
    site.ext_string()
        .split("$$$")
        .map(|s| s.trim().to_string())
        .collect()
}

/// ext 里所有 JSON 段（对象形式 ext 在前，`$$$` 文本里的 JSON 段按出现顺序在后）。
fn ext_maps(site: &Site) -> Vec<serde_json::Map<String, Value>> {
    let mut maps = Vec::new();
    if let Some(obj) = site.ext_object() {
        maps.push(obj);
    }
    for seg in ext_segments(site) {
        if !seg.starts_with('{') {
            continue;
        }
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&seg) {
            maps.push(map);
        }
    }
    maps
}

/// 取 ext JSON 段里最后一个非空字符串字段（后出现的段覆盖前面的段）。
fn ext_str(maps: &[serde_json::Map<String, Value>], key: &str) -> Option<String> {
    maps.iter().rev().find_map(|map| {
        map.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

/// 规范化基址（去空白、去尾斜杠）；空串返回空。
fn normalize_base(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

/// 伴生服务基址（测试钩子：ext JSON 的 `tgsearch_url` / `base` 可注入 mock 地址）。
///
/// 上游 `w()` 读全局配置 `global_panconfig_tgsearch_url`（本地类读
/// `..._local_url`），桌面端无全局配置 → 改读 ext JSON。缺省落
/// `http://127.0.0.1:10199`。
fn base_of(_cfg: &TvConfig, site: &Site) -> String {
    let maps = ext_maps(site);
    let keys: &[&str] = if is_local(&site.api) {
        &["tgsearch_local_url", "tgsearch_url", "base"]
    } else {
        &["tgsearch_url", "base"]
    };
    for key in keys {
        if let Some(value) = ext_str(&maps, key) {
            let base = normalize_base(&value);
            if !base.is_empty() {
                return base;
            }
        }
    }
    DEFAULT_BASE.to_string()
}

/// 搜索端点：全站 `{base}/`，本地/频道 `{base}/s/1`。
fn endpoint(cfg: &TvConfig, site: &Site) -> String {
    let base = base_of(cfg, site);
    if is_local(&site.api) {
        format!("{base}/s/1")
    } else {
        format!("{base}/")
    }
}

/// 请求头（`TGYunPan.getHeaders()`：UA + Referer=基址）。
fn request_headers(base: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Referer".to_string(), base.to_string());
    headers
}

/// 请求体：`Base64(JSON{page,encode,keyword,channelUsername})`（NO_WRAP，字段顺序照上游）。
fn request_body(site: &Site, keyword: &str, page: u32) -> Result<String> {
    let channel = ext_str(&ext_maps(site), "channellist").unwrap_or_default();
    let keyword_json = serde_json::to_string(keyword)
        .map_err(|e| parse_err(site, format!("tgsearch 关键词 JSON 编码失败: {e}")))?;
    let channel_json = serde_json::to_string(&channel)
        .map_err(|e| parse_err(site, format!("tgsearch 频道 JSON 编码失败: {e}")))?;
    let payload = format!(
        r#"{{"page":"{page}","encode":"1","keyword":{keyword_json},"channelUsername":{channel_json}}}"#
    );
    Ok(STANDARD.encode(payload.as_bytes()))
}

/// 发一次搜索请求，返回响应正文。
fn request_text(cfg: &TvConfig, site: &Site, keyword: &str, page: u32) -> Result<String> {
    let url = endpoint(cfg, site);
    let body = request_body(site, keyword, page)?;
    let client =
        client_for(&cfg.http).map_err(|e| net_err(site, format!("HTTP 客户端初始化失败: {e}")))?;
    let headers = request_headers(&base_of(cfg, site));
    post_json_text(&client, &url, &headers, &body)
        .map_err(|e| net_err(site, format!("tgsearch 请求失败（{url}）: {e}")))
}

/* ============================================================
 * 正则（照抄 TGYunPanLocal 的静态 Pattern，见 lc-deobf:49-94）
 * ============================================================ */

/// `TGYunPanLocal.B`：TG 网页预览里的图片背景。
fn photo_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"tgme_widget_message_photo_wrap.+?background-image:url\(['"]((https?:)?//([^\s<'\x22]+)\.(jpg|png|webp))"#,
        )
        .expect("TG 图片背景正则合法")
    })
}

/// `TGYunPanLocal.C`：裸图片 URL。
fn image_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(https?:)?//([^\s<]+)\.(jpg|png|webp)"#).expect("图片 URL 正则合法")
    })
}

/// `TGYunPanLocal.E`：`<strong>` 标题。
fn strong_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<strong>(.+?)</strong>").expect("strong 正则合法"))
}

/// `TGYunPanLocal.A`：「番号」。
fn number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(番号)([:：])([^\s<]+)").expect("番号正则合法"))
}

/// `TGYunPanLocal.z`：「标题/片名/名称/资源名」。
fn title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(标题|片名|名称|资源名)([:：])([^\s<]+)").expect("标题正则合法"))
}

/// `TGYunPan.t` 的网盘分享链接正则（`TGYunPanLocal.y`，按出现顺序取）。
fn pan_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"https?://(?:(?:www\.(?:alipan|aliyundrive)\.com|pan\.quark\.cn|drive\.uc\.cn)/s/\w+(?:\?public=1)?|115\.com/s/\w+\?password=\w+|caiyun\.139\.com/(w/i/([^?#]+)|m/i\?([^#]+))|pan\.xunlei\.com/s/([^?#]+)|mypikpak\.com/s/([^?#]+))"#,
        )
        .expect("网盘链接正则合法")
    })
}

/// `TGYunPanLocal.D`：magnet 链接。
fn magnet_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"magnet:[^\s<'\x22]+"#).expect("magnet 正则合法"))
}

/// `TGYunPanLocal` 去 HTML 标签（`</?[^>]+>`）。
fn tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"</?[^>]+>").expect("HTML 标签正则合法"))
}

/* ============================================================
 * 响应解析（对应上游 extractAllMessageTexts / tgSearch 条目转换）
 * ============================================================ */

/// 一条原始消息记录（`msgId$$chat$$link$$name$$image$$video`）。
#[derive(Debug, Default)]
struct RawRecord {
    chat: String,
    msg_id: String,
    link: String,
    name: String,
    image: String,
    video: String,
}

/// 行 base64 解码（对应 Android `Base64.decode(line, 2)`，兼容缺省 padding）。
fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    STANDARD
        .decode(text)
        .ok()
        .or_else(|| STANDARD_NO_PAD.decode(text).ok())
}

/// `media` 字段冒号前的部分（`12345:I` → `12345`）。
fn media_head(media: &str) -> &str {
    media.split(':').next().unwrap_or(media)
}

/// `TGYunPanLocal.extractMovieDescription`：`(名称, 番号, 封面/占位)`。
///
/// 上游顺序：z（标题/片名/名称/资源名）→ E（`<strong>`）→ F → G；由于 `e_name`
/// 无匹配时为单个空格、`isEmpty` 判断恒为假，F/G 实际不可达（见文件头偏差 5）。
fn extract_movie_description(text: &str) -> (String, String, String) {
    let mut pic = photo_re()
        .captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| " ".to_string());
    if pic.is_empty() {
        if let Some(m) = image_re().find(text) {
            pic = m.as_str().to_string();
        }
    }
    let e_name = strong_re()
        .captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| " ".to_string());
    let stripped = tag_re().replace_all(text, "");
    // 上游照搬：`(番号)([:：])([^\s<]+)` 取 group(2)，即分隔符本身。
    let number = number_re()
        .captures(&stripped)
        .and_then(|caps| caps.get(2))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    if let Some(caps) = title_re().captures(&stripped) {
        let name = caps
            .get(3)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        return (name, number, pic);
    }
    (e_name, number, pic)
}

/// `TGYunPan.t`：先按网盘链接正则、再按 magnet 正则提取（与上游同序，不去重）。
fn extract_links(text: &str) -> Vec<String> {
    let stripped = tag_re().replace_all(text, "");
    let mut links: Vec<String> = pan_link_re()
        .find_iter(&stripped)
        .map(|m| m.as_str().to_string())
        .collect();
    links.extend(
        magnet_re()
            .find_iter(&stripped)
            .map(|m| m.as_str().to_string()),
    );
    links
}

/// 解析单行消息，产出原始记录（跨行用 `seen` 去重 link）。
fn parse_line(base: &str, line: &str, seen: &mut HashSet<String>) -> Vec<RawRecord> {
    let decoded = if line.contains(' ') {
        line.to_string()
    } else {
        b64_decode(line)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| line.to_string())
    };
    if decoded.chars().count() <= 1 {
        return Vec::new();
    }
    let sep = if decoded.contains('\t') { '\t' } else { ' ' };
    let Some((chat, rest)) = decoded.split_once(sep) else {
        return Vec::new();
    };
    let Some((msg_id, rest)) = rest.split_once(sep) else {
        return Vec::new();
    };
    // 制表符分隔时，剩余部分按最后一个制表符再拆 `text` 与 `media`；
    // 空格分隔时剩余部分全部是 `text`，`media` 为空。
    let (text, media) = if sep == '\t' {
        let Some((text, media)) = rest.rsplit_once('\t') else {
            return Vec::new();
        };
        (text, media)
    } else {
        (rest, "")
    };

    let (name, number, pic) = extract_movie_description(text);
    let image = if pic.trim().is_empty() && media.contains(":I") {
        format!("{base}/i/{msg_id}/{}", media_head(media))
    } else {
        pic
    };
    let video = if media.contains(":V") {
        format!("{base}/v/{msg_id}/{}", media_head(media))
    } else {
        " ".to_string()
    };
    let display_name = if number.is_empty() {
        name
    } else {
        format!("{name} {number}")
    };
    let links = extract_links(text);
    if links.is_empty() {
        return Vec::new();
    }
    // 上游 `g1.g.e` 语义 = 非空；空则写单个空格占位。
    let name_field = if display_name.is_empty() {
        " ".to_string()
    } else {
        display_name
    };
    let mut out = Vec::new();
    for link in links {
        if !seen.insert(link.clone()) {
            continue;
        }
        out.push(RawRecord {
            chat: chat.to_string(),
            msg_id: msg_id.to_string(),
            link,
            name: name_field.clone(),
            image: image.clone(),
            video: video.clone(),
        });
    }
    // `video` 非空时，再用 video 当链接补一条记录（上游同款兜底）。
    if !video.trim().is_empty() {
        out.push(RawRecord {
            chat: chat.to_string(),
            msg_id: msg_id.to_string(),
            link: video.clone(),
            name: name_field,
            image,
            video,
        });
    }
    out
}

/// 解析响应正文 → 原始记录（先字典序排序，再从末尾往前逐行解析，对应上游）。
fn parse_records(base: &str, text: &str) -> Vec<RawRecord> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    lines.sort_unstable();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for line in lines.iter().rev() {
        out.extend(parse_line(base, line, &mut seen));
    }
    out
}

/// 截取错误预览（按字符截断，避免中文切片 panic）。
fn preview(text: &str) -> String {
    text.chars().take(120).collect()
}

/// 校验响应形状并解析（首字符是 JSON/HTML 时视为服务未启动或反代错误页）。
fn response_records(site: &Site, base: &str, text: &str) -> Result<Vec<RawRecord>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(first) = trimmed.chars().next() {
        if matches!(first, '{' | '[' | '<') {
            return Err(parse_err(
                site,
                format!(
                    "tgsearch 返回非预期内容（预期按行消息，疑似服务未启动或反代错误页）: {}",
                    preview(trimmed)
                ),
            ));
        }
    }
    Ok(parse_records(base, text))
}

/* ============================================================
 * 条目转换与公开 api（dispatching 依赖）
 * ============================================================ */

/// magnet `dn` 参数（URL 解码后拼到名称前）。
fn magnet_dn(link: &str) -> Option<String> {
    let start = link.find("&dn=")? + "&dn=".len();
    let value = link[start..].split('&').next().unwrap_or("");
    if value.is_empty() {
        return None;
    }
    let decoded = percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .to_string();
    let decoded = decoded.trim();
    if decoded.is_empty() {
        None
    } else {
        Some(decoded.to_string())
    }
}

/// 按字符截断（magnet 为 ASCII，与上游 `substring(0, 60)` 等价）。
fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 是否网盘分享链接（语义等价 `Push.f` / `Push.k` 的域名部分）。
fn is_pan_link(link: &str) -> bool {
    let link = link.to_ascii_lowercase();
    [
        "aliyundrive.com",
        "alipan.com",
        "xunlei.com",
        "quark.cn",
        "139.com",
        "115.com",
        "mypikpak.com",
    ]
    .iter()
    .any(|host| link.contains(host))
}

/// 按域名判类别（顺序照上游 `Ali.b` → … → `PikPak.b` → magnet → 群组视频）。
fn category_of(link: &str, video: &str) -> Option<&'static str> {
    let link = link.to_ascii_lowercase();
    if link.contains("aliyundrive.com") || link.contains("alipan.com") {
        return Some("阿里");
    }
    if link.contains("quark.cn") {
        return Some("夸克");
    }
    if link.contains("drive.uc.cn") {
        return Some("UC");
    }
    if link.contains("115.com") {
        return Some("115");
    }
    if link.contains("xunlei.com") {
        return Some("迅雷");
    }
    if link.contains("139.com") {
        return Some("YD");
    }
    if link.contains("mypikpak.com") {
        return Some("PikPak");
    }
    if link.starts_with("magnet:") {
        return Some("磁力");
    }
    if !video.trim().is_empty() {
        return Some("群组视频");
    }
    None
}

/// 类别固定封面（上游 `tgSearch` 的封面映射；YD / 群组视频无封面）。
fn cover_of(category: &str) -> Option<String> {
    let cover = match category {
        "阿里" => COVER_ALI,
        "夸克" => COVER_QUARK,
        "115" => COVER_115,
        "UC" => COVER_UC,
        "迅雷" => COVER_THUNDER,
        "PikPak" => COVER_PIKPAK,
        "磁力" => COVER_MAGNET,
        _ => return None,
    };
    Some(cover.to_string())
}

/// 原始记录 → 搜索条目（对应 `tgSearch` / `tgSearchChannel` 的逐条转换）。
fn items_from_records(keyword: &str, records: &[RawRecord]) -> Vec<VideoItem> {
    let mut out = Vec::new();
    let mut seq = 0u32;
    for record in records {
        let mut name = if record.name.trim().is_empty() {
            keyword.to_string()
        } else {
            record.name.clone()
        };
        let image = if record.image.trim().is_empty() {
            String::new()
        } else {
            record.image.clone()
        };
        let video = if record.video.trim().is_empty() {
            String::new()
        } else {
            record.video.clone()
        };
        let mut link = record.link.split('#').next().unwrap_or("").to_string();
        if link.starts_with("magnet:") {
            link = link.replace("&amp;", "&");
            if link.contains("&dn=") {
                if let Some(dn) = magnet_dn(&link) {
                    name = format!("{dn} {name}");
                }
            }
            link = truncate_chars(&link, MAGNET_ID_LEN);
        }
        // 过滤：网盘链接 / magnet / 群组视频（`video` 非空），否则丢弃。
        if !is_pan_link(&link) && !link.starts_with("magnet:") && video.is_empty() {
            continue;
        }
        let Some(category) = category_of(&link, &video) else {
            continue;
        };
        let cover = if image.is_empty() {
            cover_of(category)
        } else {
            Some(image)
        };
        seq += 1;
        out.push(VideoItem {
            id: link,
            name,
            cover,
            remark: Some(format!(
                "{seq}.{category}/{}/{}",
                record.chat, record.msg_id
            )),
        });
    }
    out
}

/// 调一次搜索并转成条目（`page` 以字符串交给伴生服务）。
fn search_page(cfg: &TvConfig, site: &Site, keyword: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(cfg, site);
    let text = request_text(cfg, site, keyword, page)?;
    let records = response_records(site, &base, &text)?;
    Ok(items_from_records(keyword, &records))
}

/// 分类：`homeContent` 固定一个 `JARSEARCH`（「搜索」）分类。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let _ = (cfg, site);
    Ok(vec![Category {
        type_id: CATEGORY_ALL.to_string(),
        type_name: CATEGORY_ALL_NAME.to_string(),
    }])
}

/// 分类内容：`categoryContent` 把 `type_id` 当关键词搜索（上游同款）。
///
/// `JARSEARCH` 首页是两个 Android UI 虚拟项（`JARSEARCHFOLDER` /
/// `SUBSCRIBECONFIG`），桌面端无对应交互 → 返回空；第二页起上游本来就返回空。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let type_id = type_id.trim();
    if type_id.is_empty() {
        return Err(parse_err(site, "分类 id 为空"));
    }
    if type_id.eq_ignore_ascii_case(CATEGORY_ALL) {
        return Ok(Vec::new());
    }
    if type_id.eq_ignore_ascii_case(CATEGORY_FOLDER) {
        return Err(CatvodError::new(
            ErrorClass::HostApi,
            site,
            "JARSEARCHFOLDER 依赖 Android 弹窗输入关键词，桌面端不支持；请直接使用搜索",
        )
        .into_core());
    }
    search_page(cfg, site, type_id, page.max(1))
}

/// 详情：原类继承 `Push`，网盘分享交网盘引擎；桌面端无引擎 → 单集直出链接。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let _ = cfg;
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "详情 id 为空"));
    }
    let name = display_name(id);
    Ok(VideoDetail {
        id: id.to_string(),
        name: name.clone(),
        cover: None,
        desc: Some(
            "TG 搜索直出链接：磁力/直链可直接播放；网盘分享链接需客户端用对应网盘账号打开（桌面端不做网盘解析）。"
                .to_string(),
        ),
        episodes: vec![EpisodeGroup {
            group: GROUP.to_string(),
            items: vec![Episode {
                title: name,
                url: id.to_string(),
            }],
        }],
        ..Default::default()
    })
}

/// 详情展示名：magnet 取 `dn`，其它取 URL 末段（取不到则原样返回 id）。
fn display_name(id: &str) -> String {
    if id.starts_with("magnet:") {
        return magnet_dn(id).unwrap_or_else(|| "磁力链接".to_string());
    }
    let no_query = id.split(['?', '#']).next().unwrap_or(id);
    let Some(seg) = no_query
        .rsplit('/')
        .find(|seg| !seg.is_empty() && !seg.contains(':'))
    else {
        return id.to_string();
    };
    if seg.len() > 80 {
        return id.to_string();
    }
    seg.to_string()
}

/// 搜索：`searchContent` → `tgSearch` / `tgSearchChannel`，固定第一页。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    search_page(cfg, site, keyword, 1)
}

/// 播放：原类继承 `Push`（磁力/直链直出，网盘交引擎）；桌面端统一把 id 交给客户端。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let _ = (cfg, flag);
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": id,
        "header": {},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_both_apis() {
        assert!(handles(API));
        assert!(handles(API_LOCAL));
        assert!(handles("  csp_tgyunpan  "));
        assert!(handles("  csp_tgyunpanlocal  "));
        assert!(!handles("csp_Pan"));
        assert!(!handles(""));
    }

    #[test]
    fn normalize_base_trims_trailing_slash() {
        assert_eq!(
            normalize_base(" http://127.0.0.1:10199/ "),
            "http://127.0.0.1:10199"
        );
        assert_eq!(normalize_base(""), "");
    }

    #[test]
    fn movie_description_prefers_strong_and_title_pattern() {
        let (name, number, pic) = extract_movie_description("<strong>测试资源</strong>");
        assert_eq!(name, "测试资源");
        assert_eq!(number, "");
        assert_eq!(pic, " ");
        let (name, _, _) = extract_movie_description("标题：三体 <strong>x</strong>");
        assert_eq!(name, "三体");
    }
}
