//! `csp_YingSo` 原生实现（鹰搜，网盘资源搜索，`extends Push` 的搜索型家族）。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/YingSo.java`
//!   （73 行；`searchContent` 被 jadx 跳过 4864 条指令，`detail/player` 继承 `Push`）。
//! * 站点 `_rea/samples/inventory/sites.json`：1 站 `YingSo`，
//!   ext `./lib/token.json$$$https://yingso.fun/$$$null$$$1`。
//!
//! 还原方法（本轮新建，可复用）：
//! 1. 从 `_rea/samples/library/jars/0f047745-FxzHm.jar` 取 `classes.dex`（dex-in-jar）；
//! 2. `dex2jar 2.4` 转 `ys.jar`，抽出 `com/github/catvod/spider/YingSo.class`；
//! 3. `Vineflower 1.10.1 -dgs=1` 单类反编译 → 810 行、0 失败；
//! 4. 通用解码器 `_rea/tmp/r124/dec-full.mjs`（`a.a(new byte[]{…}, new byte[]{…})`
//!    = `data[i] ^ key[i % key.len]`，遇 0 截断）→ 45/45 字符串内联，可读源码
//!    `_rea/tmp/r124/out-ys/YingSo.decoded.java`。
//!
//! 协议（原类逐段还原）：
//! * `h = "https://yingso.fun/"`；`init` 按 `$$$` 切 ext：段 0 交基类、段 1 长度 > 7
//!   时覆盖基址 `h`、段 2 长度 > 7 时作为代理（`e.z`）。
//! * 请求头：`User-Agent`（Chrome/122）+ `Referer: h`。
//! * `searchContent(kw, quick)` → `searchContent(kw, quick, "1")`：
//!   `POST {scheme}://{authority}:3000/v2/ali/search`，body
//!   `{"pageSize":30,"pageNum":page,"title":kw,"root":0,"cat":"all"}`。
//! * 响应 `data[]`：`root` 1→阿里、2→夸克、3→迅雷（其余跳过）；`cat`
//!   `ds`→电视剧、`dy`→电影、`qt`→综艺；`key` 为分享 id 尾巴；`title`；`time` 秒级。
//! * 条目：标题 `title`；备注 `{kw}/{yyyy-MM-dd}/{分类}`；url = 平台分享前缀 + `key`；
//!   封面 = 平台固定图。最后 `g.x(list)` 输出 `{"list":[…]}` 信封。
//!
//! 有意偏差（桌面端能力边界）：
//! 1. `detailContent` / `playerContent` 在原类里继承 `Push`（依赖 Android parser 引擎），
//!    桌面端没有该引擎 → 返回 `catvod.host.unsupported`（与既有网盘搜索簇同策略）；
//! 2. `proxy`（段 2）不参与请求：桌面端不做本地代理；
//! 3. 站点 `https://yingso.fun` 在 2026-11 已不可达 → 本家族只能以 mock 夹具验收。
//!
//! 现站核验：2026-11-12 探测 `https://yingso.fun/` 与 `:3000/v2/ali/search` 均失败；
//! 实现按原类协议做等价还原，保留 `#[ignore]` 真站用例的扩展点（暂无）。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::model::{Category, Site, TvConfig, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 本家族覆盖的 api。
pub const API: &str = "csp_YingSo";
/// 默认基址（ext 段 1 可覆盖）。
pub const DEFAULT_BASE: &str = "https://yingso.fun/";
/// 请求 UA（原类 `getHeaders()` 固定 Chrome/122）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 每页条数（原类 body 固定 30）。
const PAGE_SIZE: u32 = 30;
/// 搜索接口端口（原类在 host 后硬拼 `:3000`）。
const API_PORT: u16 = 3000;
/// 搜索接口路径。
const API_PATH: &str = "/v2/ali/search";

/// 平台固定封面（原类按 root 写死，逐字节还原）。
const COVER_ALI: &str = "https://inews.gtimg.com/newsapp_bt/0/13263837859/1000";
const COVER_QUARK: &str =
    "https://bkimg.cdn.bcebos.com/pic/6d81800a19d8bc3e93bd4e218c8ba61ea9d345a7";
const COVER_THUNDER: &str =
    "https://bkimg.cdn.bcebos.com/pic/d4628535e5dde7114164dfafa8efce1b9d166131";
/// 分享链接前缀（原类按 root 拼接）。
const LINK_ALI: &str = "https://www.alipan.com/s/";
const LINK_QUARK: &str = "https://pan.quark.cn/s/";
const LINK_THUNDER: &str = "https://pan.xunlei.com/s/";

/// 家族是否负责该 api（大小写不敏感；字面量写在函数体内，供审计脚本静态扫描）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

/// ext 分段（`$$$`）。
fn ext_segments(site: &Site) -> Vec<String> {
    site.ext_string()
        .split("$$$")
        .map(|s| s.trim().to_string())
        .collect()
}

/// 基址：ext 段 1 长度 > 7 且为 http(s) 时覆盖，否则用默认值（去尾 `/`）。
fn base_of(site: &Site) -> String {
    let segs = ext_segments(site);
    let raw = segs
        .get(1)
        .map(|s| s.as_str())
        .filter(|s| s.len() > 7 && s.starts_with("http"))
        .unwrap_or(DEFAULT_BASE);
    raw.trim_end_matches('/').to_string()
}

/// 请求头（原类 `getHeaders()`：UA + Referer）。
fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), format!("{base}/"));
    h
}

/// 搜索接口 URL：`{scheme}://{authority}:3000/v2/ali/search`
/// （原类 `Uri.parse(h).getScheme() + "://" + getAuthority() + ":3000/v2/ali/search"`）。
pub fn api_url(base: &str) -> String {
    let (scheme, rest) = match base.split_once("://") {
        Some((s, r)) => (s, r),
        None => ("https", base),
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    // 原类在 authority 后硬拼 `:3000`；桌面端把带端口的基址（镜像/mock）视为已指定端口，
    // 否则会拼出 `host:port:3000` 这种非法 URL。
    let target = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:{API_PORT}")
    };
    format!("{scheme}://{target}{API_PATH}")
}

/// 秒级时间戳 → (年, 月, 日)，按 UTC（原类用 `SimpleDateFormat("yyyy-MM-dd")`，
/// 桌面端不引入时区依赖，统一按 UTC 输出，避免跨时区结果漂移）。
fn ymd_from_secs(secs: i64) -> (i32, u32, u32) {
    // Howard Hinnant 的 days→civil 算法（与 C++ chrono 文档一致）。
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    ((y + i64::from(m <= 2)) as i32, m, d)
}

/// `cat` 字段 → 中文分类名（原类 `ds`/`dy`/`qt`，其余空串）。
fn category_label(cat: &str) -> &'static str {
    match cat {
        "ds" => "电视剧",
        "dy" => "电影",
        "qt" => "综艺",
        _ => "",
    }
}

/// `root` 字段 → (平台名, 分享前缀, 固定封面)。原类只认 1/2/3，其余跳过。
fn provider_of(root: i64) -> Option<(&'static str, &'static str, &'static str)> {
    match root {
        1 => Some(("阿里", LINK_ALI, COVER_ALI)),
        2 => Some(("夸克", LINK_QUARK, COVER_QUARK)),
        3 => Some(("迅雷", LINK_THUNDER, COVER_THUNDER)),
        _ => None,
    }
}

/// 解析 `data[]` 为列表项（原类逐条 `j.k/o/j/l` 构造，最后 `g.x(list)`）。
pub fn parse_items(value: &Value, keyword: &str) -> Vec<VideoItem> {
    let mut out = Vec::new();
    let Some(data) = value.get("data").and_then(|v| v.as_array()) else {
        return out;
    };
    for item in data {
        let root = item.get("root").and_then(|v| v.as_i64()).unwrap_or(0);
        let Some((_, prefix, cover)) = provider_of(root) else {
            continue;
        };
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let key = item
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if title.is_empty() || key.is_empty() {
            continue;
        }
        let cat = item.get("cat").and_then(|v| v.as_str()).unwrap_or("");
        let time = item.get("time").and_then(|v| v.as_i64()).unwrap_or(0);
        let (y, m, d) = ymd_from_secs(time);
        let label = category_label(cat);
        let remark = if label.is_empty() {
            format!("{keyword}/{y:04}-{m:02}-{d:02}")
        } else {
            format!("{keyword}/{y:04}-{m:02}-{d:02}/{label}")
        };
        let url = format!("{prefix}{key}");
        out.push(VideoItem {
            id: url.clone(),
            name: title,
            cover: Some(cover.to_string()),
            remark: Some(remark),
        });
    }
    out
}

/// 搜索：原类 `searchContent(kw, quick[, page])`（3 参重载 page 固定 `"1"`）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = api_url(&base);
    let client = crate::http::client_for(&cfg.http)?;
    let body = json!({
        "pageSize": PAGE_SIZE,
        "pageNum": 1,
        "title": keyword,
        "root": 0,
        "cat": "all",
    })
    .to_string();
    let text = crate::http::post_json_text(&client, &url, &headers(&base), &body)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("YingSo 搜索响应不是 JSON：{e}")))?;
    if value.get("data").and_then(|v| v.as_array()).is_none() {
        return Err(parse_err(site, "YingSo 搜索响应缺少 data 数组"));
    }
    Ok(parse_items(&value, keyword))
}

/// 搜索型家族没有首页分类（原类 `homeContent` 继承 `Push`，返回空）。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Vec<Category> {
    Vec::new()
}

/// 原类 `categoryContent` 继承 `Push`（不覆写），桌面 dispatch 不应走到这里。
pub fn videos(_cfg: &TvConfig, site: &Site, _cat: &str, _page: u32) -> Result<Vec<VideoItem>> {
    Err(host_err(
        site,
        "YingSo 是 Push 搜索型家族，只有 searchContent，没有分类列表",
    ))
}

/// 原类 `detailContent` 继承 `Push`，依赖 Android parser 引擎把分享链接转成线路；
/// 桌面端没有该引擎 → 明确报 host 能力缺失（与网盘搜索簇同策略）。
pub fn detail(_cfg: &TvConfig, site: &Site, _id: &str) -> Result<crate::model::VideoDetail> {
    Err(host_err(
        site,
        "YingSo 搜索结果是网盘分享链接，原类 detailContent 依赖 Android parser 引擎",
    ))
}

/// 同上：原类 `playerContent` 继承 `Push`，播放器由 Android 端 parser 接管。
pub fn player_content(
    _cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    _id: &str,
) -> Result<Value> {
    Err(host_err(
        site,
        "YingSo 播放依赖 Android parser 引擎，桌面端请直接使用搜索结果的分享链接",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Site;

    fn site(ext: &str) -> Site {
        let mut s = Site::default();
        s.api = API.to_string();
        s.ext = Some(serde_json::json!(ext));
        s
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_YingSo"));
        assert!(handles("CSP_YINGSO "));
        assert!(!handles("csp_YingSoX"));
    }

    #[test]
    fn base_comes_from_second_ext_segment() {
        let s = site("token.json$$$https://mirror.example.com/$$$null$$$1");
        assert_eq!(base_of(&s), "https://mirror.example.com");
        // 段 1 过短或不是 http → 回退默认值。
        let s = site("token.json$$$short$$$null");
        assert_eq!(base_of(&s), DEFAULT_BASE.trim_end_matches('/'));
    }

    #[test]
    fn api_url_follows_original_scheme_and_authority() {
        assert_eq!(
            api_url("https://yingso.fun"),
            "https://yingso.fun:3000/v2/ali/search"
        );
        // 带路径的镜像基址只用 scheme+authority。
        assert_eq!(
            api_url("http://mirror.example.com/prefix"),
            "http://mirror.example.com:3000/v2/ali/search"
        );
        // 没有 scheme 时按 https 处理。
        assert_eq!(api_url("yingso.fun"), "https://yingso.fun:3000/v2/ali/search");
        // 带端口的基址（镜像/mock）不再追加 :3000。
        assert_eq!(
            api_url("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080/v2/ali/search"
        );
    }

    #[test]
    fn headers_carry_ua_and_referer() {
        let h = headers("https://yingso.fun");
        assert_eq!(h.get("User-Agent").map(String::as_str), Some(UA));
        assert_eq!(
            h.get("Referer").map(String::as_str),
            Some("https://yingso.fun/")
        );
    }

    #[test]
    fn ymd_matches_civil_dates() {
        assert_eq!(ymd_from_secs(0), (1970, 1, 1));
        assert_eq!(ymd_from_secs(1_700_000_000), (2023, 11, 14));
        assert_eq!(ymd_from_secs(86_399), (1970, 1, 1));
        assert_eq!(ymd_from_secs(86_400), (1970, 1, 2));
    }

    #[test]
    fn category_labels_follow_original_map() {
        assert_eq!(category_label("ds"), "电视剧");
        assert_eq!(category_label("dy"), "电影");
        assert_eq!(category_label("qt"), "综艺");
        assert_eq!(category_label("xx"), "");
    }

    #[test]
    fn parse_items_maps_three_providers() {
        let value: Value = serde_json::from_str(
            r#"{"data":[
                {"root":1,"cat":"dy","key":"ali123","title":"阿里资源","time":1700000000},
                {"root":2,"cat":"ds","key":"quark456","title":"夸克资源","time":1700000000},
                {"root":3,"cat":"qt","key":"xl789","title":"迅雷资源","time":1700000000},
                {"root":4,"cat":"dy","key":"pk000","title":"PikPak 分支（原类无 4）","time":1700000000},
                {"root":0,"cat":"dy","key":"zero","title":"root 0 跳过","time":0}
            ]}"#,
        )
        .unwrap();
        let items = parse_items(&value, "关键字");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, "https://www.alipan.com/s/ali123");
        assert_eq!(items[0].name, "阿里资源");
        assert_eq!(items[0].cover.as_deref(), Some(COVER_ALI));
        assert_eq!(items[0].remark.as_deref(), Some("关键字/2023-11-14/电影"));
        assert_eq!(items[1].id, "https://pan.quark.cn/s/quark456");
        assert_eq!(items[1].remark.as_deref(), Some("关键字/2023-11-14/电视剧"));
        assert_eq!(items[2].id, "https://pan.xunlei.com/s/xl789");
        assert_eq!(items[2].remark.as_deref(), Some("关键字/2023-11-14/综艺"));
    }

    #[test]
    fn parse_items_skips_empty_fields_and_missing_data() {
        let value: Value =
            serde_json::from_str(r#"{"data":[{"root":1,"key":"","title":"x","time":0}]}"#).unwrap();
        assert!(parse_items(&value, "k").is_empty());
        let value: Value = serde_json::from_str(r#"{"no_data":[]}"#).unwrap();
        assert!(parse_items(&value, "k").is_empty());
    }

    #[test]
    fn search_only_family_reports_host_limits() {
        let s = site("");
        let cfg = TvConfig::default();
        assert!(categories(&cfg, &s).is_empty());
        let e = videos(&cfg, &s, "1", 1).unwrap_err();
        assert!(e.to_string().contains("catvod.host.unsupported"));
        let e = detail(&cfg, &s, "https://pan.quark.cn/s/x").unwrap_err();
        assert!(e.to_string().contains("catvod.host.unsupported"));
        let e = player_content(&cfg, &s, "网盘", "https://pan.quark.cn/s/x").unwrap_err();
        assert!(e.to_string().contains("catvod.host.unsupported"));
    }
}
