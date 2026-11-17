//! `csp_AliShare`（阿里分享列表型蜘蛛）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/samples/library/jars/0f047745-FxzHm.jar` 的
//!   `com.github.catvod.spider.AliShare`，`jadx --show-bad-code --single-class`
//!   恢复为 `_rea/tmp/r146/jadx/AliShare.bad.java`（833 行、0 skip）。
//! * 字符串全部由 `merge.z.C0338a.a(byte[], byte[])` 加密（字节循环异或 key
//!   后 UTF-8，见 `.../parser/merge/Z/C0338a.java`），`_rea/tmp/r146/dec-alishare.mjs`
//!   解出 86 条（剩 12 处 SQL 列名/分隔符为运行期拼接）。
//!
//! 协议（逐段还原）：
//! * `init(Context, String)`：ext 按 `$$$` 拆：
//!   [0] 交 `super.init`（Ali proxy/token 配置）；[1] 列表源；[2] `db`；
//!   [3] `"1"` 缓存标志。[1] 为 `./` 相对路径时，原类会拼到 [0]（Ali 配置）
//!   的目录再 GET；仅当拼接结果是 http 才发请求，否则直接当内容用。
//! * 列表源两种形态：
//!   1. JSON 数组 `[{type_id, type_name, type_flag}]`（`merge.B.b.a`），
//!      `type_id` 不以 http 开头时补 `https://www.alipan.com/s/` 前缀；
//!   2. 纯文本，逐行按 `[\t\s]+` 切分取 `[shareToken, name]`，同样补前缀。
//!   原类以此构建分类列表（`d`）与 `id → 名字` 映射（`g`）。
//! * `homeContent`：返回分类 + `filters.orderby`（`name` / `updated_at`，
//!   各带 ASC/DESC）。桌面端 `Category` 无筛选 UI，排序参数按默认处理。
//! * `categoryContent(shareUrl, page, z, filters)`：解析分享链接后调用
//!   `E.D().f0(share_id, pwd, parent, files, dirs, marker, sort, false)` 浏览
//!   分享目录；条目的 `id` 由引擎按「分享 id + 目录 + `_FN_` + 文件名 +
//!   `|_FILE`」编码，`name` 为文件名、`remark` 为更新时间、`type` 为
//!   `folder`/`file`；`page` 与 `marker` 分页。
//! * `detailContent` / `playerContent` 未覆写 → 继承 Ali 引擎（分享内视频作为
//!   剧集，播放经引擎解析为直链）。`searchContent` → `searchContent_Disable`
//!   （恒返回空），因此桌面端 `search` 也返回空列表。
//!
//! 有意偏差（桌面端能力边界 / 上游现状）：
//! 1. 原类 ext 列表源是相对路径（`./json/alishare.txt`），依赖宿主 config
//!    基址解析；桌面端 `Site` 无 config 字段 → 相对路径不可解析。可用 ext 里
//!    JSON 段钩子 `list_source`（内联内容）/`list_url`（http 地址）注入，
//!    两者都没有时返回 `catvod.result.parse` 并说明原因。
//! 2. 原类以引擎 marker 翻页；桌面端每次只取首页（`videos(page>1)` 返回空），
//!    分享条目较多时需客户端下拉翻页，记偏差。
//! 3. `db` 段（SQLite 缓存）与缓存标志忽略：桌面端不做磁盘缓存。
//! 4. 分类条目 `type_flag` 原类保留但 UI 未用，桌面端忽略。
//! 5. 详情线路名固定为「阿里云盘」（原类由引擎生成），播放头补 UA/Referer。

use std::collections::HashMap;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::{parse_share_link, PanEntry};

use super::super::contract::{CatvodError, ErrorClass};
use super::yunpan::{ali_client, browse_share, load_pan_config};

/// 本家族覆盖的 api。
pub const API: &str = "csp_AliShare";

/// 阿里云盘分享前缀（原类 `C0338a` 解出的固定串）。
const SHARE_PREFIX: &str = "https://www.alipan.com/s/";

/// 详情线路名（原类由引擎生成，桌面端固定）。
const GROUP: &str = "阿里云盘";

/// 播放请求头 UA（原类走引擎，桌面端固定桌面浏览器 UA）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

/// 播放请求头 Referer。
const REFERER: &str = "https://www.alipan.com/";

/// 列表源条数上限（防止脏配置导致内存暴涨）。
const MAX_ENTRIES: usize = 500;

/// 是否本家族覆盖的 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// 统一构造 `catvod.result.parse` 错误。
fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// 把 ext 按 `$$$` 拆成段（去首尾空白）。
fn ext_segments(site: &Site) -> Vec<String> {
    site.ext_string()
        .split("$$$")
        .map(|s| s.trim().to_string())
        .collect()
}

/// ext JSON 段里的桌面端钩子（相对列表源在桌面端不可解析，需要注入）。
#[derive(Debug, Default, serde::Deserialize)]
struct Hooks {
    #[serde(default)]
    list_source: String,
    #[serde(default)]
    list_url: String,
}

/// 读取 ext 中所有 JSON 段的钩子（后出现者覆盖前者）。
fn hooks(site: &Site) -> Hooks {
    let mut out = Hooks::default();
    for seg in ext_segments(site) {
        if !seg.starts_with('{') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&seg) else {
            continue;
        };
        if let Some(s) = v.get("list_source").and_then(|x| x.as_str()) {
            out.list_source = s.to_string();
        }
        if let Some(s) = v.get("list_url").and_then(|x| x.as_str()) {
            out.list_url = s.to_string();
        }
    }
    out
}

/// 补全分享链接前缀（原类对非 http 的 `type_id` 统一补 `alipan.com/s/`）。
fn normalize_share(raw: &str) -> String {
    let s = raw.trim();
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("{SHARE_PREFIX}{}", s.trim_start_matches('/'))
    }
}

/// 是否是内联内容（JSON）。
fn is_inline(s: &str) -> bool {
    s.starts_with('{') || s.starts_with('[')
}

/// 拉取 http 列表源。
fn fetch_source(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    let headers = HashMap::from([("User-Agent".to_string(), UA.to_string())]);
    get_text(&client, url, &headers, 2)
}

/// 取列表源内容：ext[1] 内联 → 直接用；http → 拉取；相对路径 → 桌面端钩子。
fn list_source(cfg: &TvConfig, site: &Site) -> Result<String> {
    let segs = ext_segments(site);
    let raw = segs.get(1).cloned().unwrap_or_default();
    if is_inline(&raw) {
        return Ok(raw);
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return fetch_source(cfg, &raw)
            .map_err(|e| parse_err(site, format!("拉取列表源失败: {e}")));
    }
    let h = hooks(site);
    if !h.list_source.trim().is_empty() {
        return Ok(h.list_source);
    }
    if h.list_url.starts_with("http://") || h.list_url.starts_with("https://") {
        return fetch_source(cfg, &h.list_url)
            .map_err(|e| parse_err(site, format!("拉取列表源失败: {e}")));
    }
    Err(parse_err(
        site,
        format!(
            "列表源为相对路径 `{raw}`，桌面端无法解析宿主 config 基址；\
             可用 ext JSON 钩子 list_source/list_url 注入（见模块头偏差 1）"
        ),
    ))
}

/// 从 JSON 对象里按候选键取字符串。
fn field_str(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
        if let Some(n) = v.get(*k).and_then(|x| x.as_i64()) {
            return n.to_string();
        }
    }
    String::new()
}

/// 解析列表源 → `[(share_url, name)]`（顺序即原类分类顺序）。
fn parse_entries(text: &str) -> Vec<(String, String)> {
    let t = text.trim();
    let mut out: Vec<(String, String)> = Vec::new();
    if t.starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(t) {
            for item in arr {
                push_entry(&mut out, &item);
            }
        }
        return out;
    }
    if t.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            let arr = v
                .get("data")
                .or_else(|| v.get("list"))
                .or_else(|| v.get("items"));
            if let Some(arr) = arr.and_then(|x| x.as_array()) {
                for item in arr {
                    push_entry(&mut out, item);
                }
            }
        }
        return out;
    }
    for line in t.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split(['\t', ' ']).filter(|s| !s.is_empty());
        let Some(id) = parts.next() else { continue };
        let name = parts.next().unwrap_or(id);
        out.push((normalize_share(id), name.to_string()));
        if out.len() >= MAX_ENTRIES {
            break;
        }
    }
    out
}

fn push_entry(out: &mut Vec<(String, String)>, item: &Value) {
    if out.len() >= MAX_ENTRIES {
        return;
    }
    if let Some(s) = item.as_str() {
        out.push((normalize_share(s), s.to_string()));
        return;
    }
    let id = field_str(item, &["type_id", "shareToken", "url", "id", "key"]);
    if id.is_empty() {
        return;
    }
    let name = field_str(item, &["type_name", "name", "title"]);
    let name = if name.is_empty() {
        id.clone()
    } else {
        name
    };
    out.push((normalize_share(&id), name));
}

/// 浏览分享目录（`sub_fid=None` 为根目录）。
fn browse_dir(
    cfg: &TvConfig,
    site: &Site,
    share: &str,
    sub_fid: Option<&str>,
) -> Result<Vec<PanEntry>> {
    let mut link = parse_share_link(share)
        .ok_or_else(|| parse_err(site, format!("不是可识别的网盘分享链接: {share}")))?;
    if let Some(fid) = sub_fid {
        link.sub_fid = fid.to_string();
    }
    let conf = load_pan_config(site);
    browse_share(cfg, &conf, &link).map_err(|e| parse_err(site, format!("浏览分享失败: {e}")))
}

/// 条目引用编码：`{u: 分享, f: fid, n: 名称, d: 是否目录}` → URL-safe base64。
fn encode_ref(share: &str, fid: &str, name: &str, is_dir: bool) -> String {
    let v = json!({ "u": share, "f": fid, "n": name, "d": is_dir }).to_string();
    URL_SAFE_NO_PAD.encode(v.as_bytes())
}

/// 解析条目引用；非法/缺字段返回 `catvod.result.parse`。
fn decode_ref(site: &Site, id: &str) -> Result<(String, String, String, bool)> {
    let bytes = URL_SAFE_NO_PAD
        .decode(id.trim())
        .map_err(|e| parse_err(site, format!("条目 id 不是合法引用: {e}")))?;
    let v: Value = serde_json::from_slice(&bytes)
        .map_err(|e| parse_err(site, format!("条目引用 JSON 非法: {e}")))?;
    let share = field_str(&v, &["u"]);
    let fid = field_str(&v, &["f"]);
    if share.is_empty() || fid.is_empty() {
        return Err(parse_err(site, "条目引用缺少分享/文件 id"));
    }
    let name = field_str(&v, &["n"]);
    let is_dir = v.get("d").and_then(|x| x.as_bool()).unwrap_or(false);
    Ok((share, fid, name, is_dir))
}

/// 字节数转人类可读（备注用）。
fn size_text(size: i64) -> String {
    if size <= 0 {
        return String::new();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", size, units[0])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// 网盘条目 → `VideoItem`（目录用 📁 前缀）。
fn item_of(share: &str, e: &PanEntry) -> VideoItem {
    let name = if e.is_dir {
        format!("📁{}", e.name)
    } else {
        e.name.clone()
    };
    let remark = if e.is_dir {
        String::new()
    } else {
        size_text(e.size)
    };
    VideoItem {
        id: encode_ref(share, &e.fid, &e.name, e.is_dir),
        name,
        cover: if e.thumbnail.is_empty() {
            None
        } else {
            Some(e.thumbnail.clone())
        },
        remark: if remark.is_empty() { None } else { Some(remark) },
        ..Default::default()
    }
}

/// 分类 = 列表源条目（分享即分类，值域与原类 `d` 一致）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let text = list_source(cfg, site)?;
    Ok(parse_entries(&text)
        .into_iter()
        .map(|(id, name)| Category {
            type_id: id,
            type_name: name,
        })
        .collect())
}

/// 分类内容 = 浏览分类分享的根目录（目录/视频条目）。原类靠引擎 marker 翻页，
/// 桌面端只取首页。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
) -> Result<Vec<VideoItem>> {
    if page > 1 {
        return Ok(Vec::new());
    }
    let entries = browse_dir(cfg, site, type_id, None)?;
    Ok(entries.iter().map(|e| item_of(type_id, e)).collect())
}

/// 详情：目录 → 展开其中的视频为剧集；文件 → 单集。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let (share, fid, name, is_dir) = decode_ref(site, id)?;
    let mut items: Vec<Episode> = Vec::new();
    if is_dir {
        let entries = browse_dir(cfg, site, &share, Some(&fid))?;
        for e in entries.iter().filter(|e| !e.is_dir) {
            items.push(Episode {
                title: e.name.clone(),
                url: format!("{share}#{}", e.fid),
            });
        }
    } else {
        items.push(Episode {
            title: if name.is_empty() {
                fid.clone()
            } else {
                name.clone()
            },
            url: format!("{share}#{fid}"),
        });
    }
    if items.is_empty() {
        return Err(parse_err(site, format!("分享内没有可播放视频: {share}")));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name: name.clone(),
        cover: None,
        desc: None,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: GROUP.to_string(),
            items,
        }],
        ..Default::default()
    })
}

/// 播放：`{share}#{fid}` → 阿里分享 token → `play_url`（失败回退 `download_url`）。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let (share, fid) = id
        .split_once('#')
        .ok_or_else(|| parse_err(site, format!("播放 id 缺少 # 分隔: {id}")))?;
    let link = parse_share_link(share)
        .ok_or_else(|| parse_err(site, format!("不是可识别的网盘分享链接: {share}")))?;
    let conf = load_pan_config(site);
    let c = ali_client(cfg, &conf)?;
    let token = c
        .share_token(&link.share_id, &link.passcode)
        .map_err(|e| parse_err(site, format!("获取分享 token 失败: {e}")))?;
    let url = match c.play_url(&link.share_id, fid, &token.share_token) {
        Ok(u) if !u.trim().is_empty() => u,
        _ => c
            .download_url(&link.share_id, fid, &token.share_token)
            .map_err(|e| parse_err(site, format!("获取播放地址失败: {e}")))?,
    };
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": { "User-Agent": UA, "Referer": REFERER },
    }))
}

/// 原类 `searchContent` 恒返回空（`searchContent_Disable`），桌面端保持一致。
pub fn search(_cfg: &TvConfig, _site: &Site, _keyword: &str) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        Site {
            key: "alishare".to_string(),
            name: "影视分享".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(Value::String(ext.to_string())),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_AliShare"));
        assert!(handles("  csp_alishare  "));
        assert!(!handles("csp_XiaoZhiTiao"));
    }

    #[test]
    fn normalize_share_adds_prefix_and_keeps_http() {
        assert_eq!(
            normalize_share("abc123"),
            "https://www.alipan.com/s/abc123"
        );
        assert_eq!(normalize_share("/abc123"), "https://www.alipan.com/s/abc123");
        assert_eq!(
            normalize_share("https://www.aliyundrive.com/s/x"),
            "https://www.aliyundrive.com/s/x"
        );
    }

    #[test]
    fn parse_entries_reads_json_array() {
        let text = r#"[{"type_id":"tokA","type_name":"影视"},{"type_id":"https://www.alipan.com/s/tokB","type_name":"书籍"}]"#;
        let got = parse_entries(text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "https://www.alipan.com/s/tokA");
        assert_eq!(got[0].1, "影视");
        assert_eq!(got[1].0, "https://www.alipan.com/s/tokB");
    }

    #[test]
    fn parse_entries_reads_text_lines() {
        let text = "tokA\t影视\n  tokB   书籍  \n# comment\n\n";
        let got = parse_entries(text);
        assert_eq!(
            got,
            vec![
                ("https://www.alipan.com/s/tokA".to_string(), "影视".to_string()),
                ("https://www.alipan.com/s/tokB".to_string(), "书籍".to_string()),
            ]
        );
    }

    #[test]
    fn parse_entries_reads_object_data() {
        let text = r#"{"data":[{"shareToken":"tokC","name":"合集"}]}"#;
        let got = parse_entries(text);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "https://www.alipan.com/s/tokC");
        assert_eq!(got[0].1, "合集");
    }

    #[test]
    fn encode_decode_ref_round_trip() {
        let share = "https://www.alipan.com/s/abc123";
        let id = encode_ref(share, "fid-1", "第 1 集.mkv", false);
        let (u, f, n, d) = decode_ref(&site(""), &id).expect("decode");
        assert_eq!(u, share);
        assert_eq!(f, "fid-1");
        assert_eq!(n, "第 1 集.mkv");
        assert!(!d);

        let dir = encode_ref(share, "dir-9", "合集", true);
        let (_, _, _, d) = decode_ref(&site(""), &dir).expect("decode dir");
        assert!(d);
    }

    #[test]
    fn decode_ref_rejects_bad_base64() {
        let err = decode_ref(&site(""), "!!!not-base64!!!").expect_err("must fail");
        assert!(format!("{err}").contains("catvod"), "err={err}");
    }

    #[test]
    fn decode_ref_rejects_missing_fields() {
        let id = URL_SAFE_NO_PAD.encode(br#"{"f":"only"}"#);
        let err = decode_ref(&site(""), &id).expect_err("must fail");
        assert!(format!("{err}").contains("分享/文件 id"), "err={err}");
    }

    #[test]
    fn size_text_formats_units() {
        assert_eq!(size_text(0), "");
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(2048), "2.0 KB");
        assert_eq!(size_text(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn item_of_marks_directories_and_encodes_ref() {
        let share = "https://www.alipan.com/s/abc";
        let dir = PanEntry {
            fid: "d1".to_string(),
            name: "剧集".to_string(),
            is_dir: true,
            size: 0,
            thumbnail: String::new(),
        };
        let item = item_of(share, &dir);
        assert_eq!(item.name, "📁剧集");
        assert!(item.remark.is_none());
        let (u, f, _, is_dir) = decode_ref(&site(""), &item.id).expect("decode");
        assert_eq!(u, share);
        assert_eq!(f, "d1");
        assert!(is_dir);

        let file = PanEntry {
            fid: "f1".to_string(),
            name: "a.mp4".to_string(),
            is_dir: false,
            size: 1024,
            thumbnail: "https://img/x.jpg".to_string(),
        };
        let item = item_of(share, &file);
        assert_eq!(item.remark.as_deref(), Some("1.0 KB"));
        assert_eq!(item.cover.as_deref(), Some("https://img/x.jpg"));
    }

    #[test]
    fn categories_uses_ext_inline_source() {
        let s = site("t$$$[{\"type_id\":\"shareA\",\"type_name\":\"影视\"}]$$$db$$$1");
        let cats = categories(&TvConfig::default(), &s).expect("categories");
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].type_id, "https://www.alipan.com/s/shareA");
        assert_eq!(cats[0].type_name, "影视");
    }

    #[test]
    fn list_source_relative_path_reports_deviation() {
        let s = site("./lib/token.json$$$./json/alishare.txt$$$db$$$1");
        let err = list_source(&TvConfig::default(), &s).expect_err("must fail");
        let text = format!("{err}");
        assert!(text.contains("catvod"), "err={text}");
        assert!(text.contains("list_source"), "err={text}");
    }

    #[test]
    fn videos_page_gt_one_is_empty_and_search_disabled() {
        let s = site("t$$$[{\"type_id\":\"shareA\",\"type_name\":\"影视\"}]$$$db$$$1");
        let cfg = TvConfig::default();
        assert!(videos(&cfg, &s, "https://www.alipan.com/s/shareA", 2)
            .expect("videos")
            .is_empty());
        assert!(search(&cfg, &s, "关键词").expect("search").is_empty());
    }
}
