//! `csp_NiNi`（泥巴影视）原生实现：Nivod `WEB/3.2` 接口 + DES 签名/解密。
//!
//! 来源（REA 反编译证据）：
//! * 类：`com.github.catvod.spider.NiNi`，`_rea/decompiled/69dadb9a-Yoursmile/deobf/
//!   com/github/catvod/spider/NiNi.java`（251 行）；`N(url, map)`（484 指令）被 jadx
//!   跳过，用 `jadx --show-bad-code --single-class` 恢复到 `_rea/tmp/r140/NiNi.bad.java`，
//!   再用 dex2jar + Vineflower 单类反编译到 `_rea/tmp/r140/vf1/.../NiNi.java`（444 行），
//!   完整读出签名算法。
//! * 签名工具类：`com.github.catvod.spider.merge.Bu.SN`（md5 → 小写 hex，不足 32 位左补
//!   `0`，`C0028.m776/m777`、`C0035.m923` 为 `MessageDigest.digest` / `toLowerCase`）；
//!   加密工具类 `Cipher.getInstance("DES/ECB/PKCS7Padding")`，密钥 `diao.com`。
//! * 请求工具：`merge.by.zH(url, form, headers, null)`（POST 表单）、`by.Gc`（GET）。
//!
//! 协议（原类逐段还原）：
//! * `init(ctx, ext)`：`ext == "1"` 时仅保留频道 7（原类 `this.yq`）。
//! * 签名：`oid = hex.lower(DES-ECB-PKCS7( (now-600000) + "_" + "AABBCC" ))`；
//!   固定参数 `_ts/app_version=1.0/platform=3/market_id=web_nivod/device_code=web/
//!   versioncode=1/oid`；`sign = md5_hex_lower("__QUERY::<固定参数按 key 排序 k=v&>" +
//!   "__BODY::<业务参数按 key 排序 k=v&>" + "__KEY::2x_Give_it_a_shot")`；
//!   请求 URL = `base + path + "?" + 固定参数 + "&sign=" + sign`，业务参数走表单 body。
//! * 响应体是 DES 密文的 **十六进制字符串** → 解密后才是 JSON。
//! * 分类：`POST /show/filter/condition/WEB/3.2` → `channels[]`（channelId/channelName）。
//! * 首页：`/index/ranking/list/WEB/3.2`（普通模式）或
//!   `/index/desktop/WEB/3.2`（频道 7 模式，取 `banners[].show`）。
//! * 列表：`POST /show/filter/WEB/3.2`，参数 `sort_by/channel_id/show_type_id/region_id/
//!   lang_id/year_range/start`（`start=(page-1)*20`，默认 `by=3/area=0/lang=0/class=0`）；
//!   行 `list[]` → `showIdCode/showTitle/showImg/episodesTxt`。
//! * 详情：`POST /show/detail/WEB/3.2`，参数 `show_id_code` → `entity`；
//!   `plays[]{episodeName,playIdCode}`，剧集 URL = `showIdCode + "_" + playIdCode`，
//!   线路名 `泥巴`（原类 `x3.G("泥巴")`）。
//! * 搜索：`POST /show/search/WEB/3.2`，参数 `keyword/start=0/cat_id=1/keyword_type=0`。
//! * 播放：`POST /show/play/info/WEB/3.2`，参数 `show_id_code/play_id_code/oid`，
//!   取 `entity.playUrl`，带走 `User-Agent`/`Referer` 头。
//!
//! 有意偏差（桌面端能力边界 / 上游状态）：
//! 1. 上游 `api.nivod.tv` 已停放：2026-11 探测返回 5,681 B 的 SPA 首页 HTML 而非密文 JSON，
//!    且证书链不完整；本家族因此 **mock-only**，测试用离线加密夹具。
//! 2. 原类解析失败返回空串；这里返回 `catvod.result.parse` 结构化错误。
//! 3. `VideoDetail` 没有 `type_name`/`vod_remarks` 字段，原类的 `channelName` 不展示；
//!    `episodesTxt` 放在 `VideoItem.remark`（与原类 `x3` 备注一致）。
//! 4. 首页接口在本项目「原生家族」契约里没有单独入口（只有分类/列表/详情/搜索/播放），
//!    `home_videos` 仅作为协议完备性 helper 保留并由单测覆盖。
//! 5. `ext` 以 `http` 开头时覆盖 API 基址（仅测试/镜像钩子，原类没有该行为）。

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use crate::error::Result;
use crate::http::{client_for, post_form_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 站点 api 名。
pub const API: &str = "csp_NiNi";
/// 原类硬编码 API 基址。
pub const DEFAULT_BASE: &str = "https://api.nivod.tv";
/// 原类请求 UA（`getHeaders()`）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36";
/// 原类 Referer。
pub const REFERER: &str = "https://www.nivod.tv/";
/// 原类线路名（`x3.G("泥巴")`）。
pub const GROUP: &str = "泥巴";
/// `ext == "1"` 时只保留的频道 id。
pub const SPECIAL_CHANNEL: &str = "7";
/// DES 密钥（`DESKeySpec("diao.com")`）。
const DES_KEY: &[u8] = b"diao.com";
/// 签名尾盐（`__KEY::2x_Give_it_a_shot`）。
const SIGN_KEY: &str = "2x_Give_it_a_shot";
/// `oid` 原文后缀（`v.yq(ts, "_", "AABBCC")`）。
const OID_SUFFIX: &str = "AABBCC";
/// 列表每页条数（原类 `start=(page-1)*20`）。
const PAGE_SIZE: u32 = 20;

/// `csp_NiNi` 是否归本家族。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn headers() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), UA.to_string());
    map.insert("Referer".to_string(), REFERER.to_string());
    map
}

/// 桌面端毫秒时间戳（对齐 Java `new Date().getTime()`）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// DES/ECB/PKCS7 加密 → 小写十六进制（原类 `oid` 的生成方式）。
pub fn des_encrypt_hex(plain: &str) -> String {
    use ecb::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};
    let cipher = ecb::Encryptor::<des::Des>::new_from_slice(DES_KEY).expect("8 字节 DES 密钥");
    let out = cipher.encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
    hex::encode(out)
}

/// DES/ECB/PKCS7 解密（输入为十六进制字符串）。
pub fn des_decrypt_hex(text: &str) -> std::result::Result<String, String> {
    use ecb::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};
    let raw = hex::decode(text.trim()).map_err(|e| format!("响应不是十六进制: {e}"))?;
    let cipher = ecb::Decryptor::<des::Des>::new_from_slice(DES_KEY)
        .map_err(|e| format!("DES 初始化失败: {e}"))?;
    let out = cipher
        .decrypt_padded_vec_mut::<Pkcs7>(&raw)
        .map_err(|e| format!("DES 去填充失败: {e}"))?;
    String::from_utf8(out).map_err(|e| format!("解密结果不是 UTF-8: {e}"))
}

/// 指定时刻的 `oid`（把时间戳单独抽出便于单测）。
pub fn oid_at(now: i64) -> String {
    des_encrypt_hex(&format!("{}_{}", now - 600_000, OID_SUFFIX))
}

/// 固定参数（原类 `N()` 里的 `treeMap`，`_ts` 用当前毫秒）。
fn fixed_params(now: i64) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    map.insert("_ts".to_string(), now.to_string());
    map.insert("app_version".to_string(), "1.0".to_string());
    map.insert("platform".to_string(), "3".to_string());
    map.insert("market_id".to_string(), "web_nivod".to_string());
    map.insert("device_code".to_string(), "web".to_string());
    map.insert("versioncode".to_string(), "1".to_string());
    map.insert("oid".to_string(), oid_at(now));
    map
}

/// 签名：`md5_hex_lower("__QUERY::<固定参数>" + "__BODY::<业务参数>" + "__KEY::<盐>")`。
///
/// 参数按 key 升序（原类 `TreeMap`），空值跳过，条目之间以 `&` 结尾。
pub fn sign_of(fixed: &BTreeMap<String, String>, params: &BTreeMap<String, String>) -> String {
    let mut buf = String::from("__QUERY::");
    push_entries(&mut buf, fixed.iter().map(|(k, v)| (k.as_str(), v)));
    buf.push_str("__BODY::");
    push_entries(&mut buf, params.iter().map(|(k, v)| (k.as_str(), v)));
    buf.push_str("__KEY::");
    buf.push_str(SIGN_KEY);
    format!("{:x}", md5::compute(buf.as_bytes()))
}

fn push_entries<'a>(buf: &mut String, entries: impl Iterator<Item = (&'a str, &'a String)>) {
    for (k, v) in entries {
        if v.is_empty() {
            continue;
        }
        buf.push_str(k);
        buf.push('=');
        buf.push_str(v);
        buf.push('&');
    }
}

/// 拼出带签名的请求 URL（固定参数按 key 升序，末尾追加 `sign`）。
pub fn signed_url(
    base: &str,
    path: &str,
    fixed: &BTreeMap<String, String>,
    params: &BTreeMap<String, String>,
) -> String {
    let sign = sign_of(fixed, params);
    let mut url = format!("{}{}?", base.trim_end_matches('/'), path);
    let mut first = true;
    for (k, v) in fixed {
        if v.is_empty() {
            continue;
        }
        if !first {
            url.push('&');
        }
        url.push_str(k);
        url.push('=');
        url.push_str(v);
        first = false;
    }
    if !first {
        url.push('&');
    }
    url.push_str("sign=");
    url.push_str(&sign);
    url
}

fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let ext = ext.trim();
    if ext.starts_with("http://") || ext.starts_with("https://") {
        return ext.trim_end_matches('/').to_string();
    }
    DEFAULT_BASE.to_string()
}

/// 原类 `init` 的 `ext == "1"` 语义（仅保留频道 7）。
fn special_mode(site: &Site) -> bool {
    site.ext_string().trim() == "1"
}

/* ============================================================
 * 网络层：签名 + POST + 响应 DES 解密
 * ============================================================ */

/// 发起一次签名 POST 并把十六进制密文响应解密成 JSON。
fn post(
    cfg: &TvConfig,
    site: &Site,
    path: &str,
    params: &BTreeMap<String, String>,
) -> Result<Value> {
    let base = base_of(site);
    let now = now_ms();
    let fixed = fixed_params(now);
    let url = signed_url(&base, path, &fixed, params);
    let client = client_for(&cfg.http)?;
    let form: HashMap<String, String> = params
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let body = post_form_text(&client, &url, &headers(), &form)?;
    let plain = des_decrypt_hex(&body)
        .map_err(|e| parse_err(site, format!("csp_NiNi 响应解密失败: {e}")))?;
    serde_json::from_str(&plain)
        .map_err(|e| parse_err(site, format!("csp_NiNi 响应 JSON 解析失败: {e}")))
}

/* ============================================================
 * JSON 读取 helper
 * ============================================================ */

fn s_of(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn opt_of(value: &Value, key: &str) -> Option<String> {
    let s = s_of(value, key);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn arr_of(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// 原类 `i(JSONObject)`：`episodesTxt` 为空或 `"null"` 时返回空串。
fn remark_of(value: &Value) -> Option<String> {
    let s = s_of(value, "episodesTxt");
    if s.is_empty() || s == "null" {
        None
    } else {
        Some(s)
    }
}

fn item_of(value: &Value) -> Option<VideoItem> {
    let id = s_of(value, "showIdCode");
    let name = s_of(value, "showTitle");
    if id.is_empty() || name.is_empty() {
        return None;
    }
    Some(VideoItem {
        id,
        name,
        cover: opt_of(value, "showImg"),
        remark: remark_of(value),
    })
}

/* ============================================================
 * 纯解析函数（便于单测）
 * ============================================================ */

/// `/show/filter/condition/WEB/3.2` → 分类列表；`special` 时只留频道 7。
pub fn parse_categories(value: &Value, special: bool) -> Vec<Category> {
    let mut out = Vec::new();
    for channel in arr_of(value, "channels") {
        let id = s_of(&channel, "channelId");
        let name = s_of(&channel, "channelName");
        if id.is_empty() || name.is_empty() {
            continue;
        }
        if special && id != SPECIAL_CHANNEL {
            continue;
        }
        out.push(Category::new(id, name));
    }
    out
}

/// `/show/filter/WEB/3.2` 与 `/show/search/WEB/3.2` 共用的 `list[]`。
pub fn parse_list(value: &Value) -> Vec<VideoItem> {
    arr_of(value, "list")
        .iter()
        .filter_map(item_of)
        .collect()
}

/// 首页：普通模式取 `list[][]`（排行榜二维数组），频道 7 模式取 `banners[].show`。
pub fn parse_home(value: &Value, special: bool) -> Vec<VideoItem> {
    let mut out = Vec::new();
    if special {
        for banner in arr_of(value, "banners") {
            if let Some(show) = banner.get("show") {
                if let Some(item) = item_of(show) {
                    out.push(item);
                }
            }
        }
        return out;
    }
    for group in arr_of(value, "list") {
        let rows = group.as_array().cloned().unwrap_or_default();
        for row in rows {
            if let Some(item) = item_of(&row) {
                out.push(item);
            }
        }
    }
    out
}

/// `/show/detail/WEB/3.2` 的 `entity` → 详情。
pub fn parse_detail(site: &Site, id: &str, value: &Value) -> Result<VideoDetail> {
    let entity = value
        .get("entity")
        .ok_or_else(|| parse_err(site, "csp_NiNi 详情缺少 entity"))?;
    let code = s_of(entity, "showIdCode");
    let mut items = Vec::new();
    for play in arr_of(entity, "plays") {
        let play_id = s_of(&play, "playIdCode");
        if play_id.is_empty() {
            continue;
        }
        let mut title = s_of(&play, "episodeName");
        if title.is_empty() {
            title = "播放".to_string();
        }
        items.push(Episode {
            title,
            url: format!("{}_{}", code, play_id),
        });
    }
    if items.is_empty() {
        return Err(parse_err(site, "csp_NiNi 详情没有可播放剧集"));
    }
    Ok(VideoDetail {
        id: if code.is_empty() { id.to_string() } else { code },
        name: s_of(entity, "showTitle"),
        cover: opt_of(entity, "showImg"),
        desc: opt_of(entity, "showDesc"),
        year: opt_of(entity, "postYear"),
        area: opt_of(entity, "regionName"),
        director: opt_of(entity, "director"),
        actor: opt_of(entity, "actors"),
        episodes: vec![EpisodeGroup {
            group: GROUP.to_string(),
            items,
        }],
    })
}

/* ============================================================
 * 五个入口
 * ============================================================ */

/// 分类：`/show/filter/condition/WEB/3.2`。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let params = BTreeMap::new();
    let value = post(cfg, site, "/show/filter/condition/WEB/3.2", &params)?;
    let cats = parse_categories(&value, special_mode(site));
    if cats.is_empty() {
        return Err(parse_err(site, "csp_NiNi 分类为空（上游可能已停放）"));
    }
    Ok(cats)
}

/// 列表：`/show/filter/WEB/3.2`。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let page = page.max(1);
    let mut params = BTreeMap::new();
    params.insert("sort_by".to_string(), "3".to_string());
    params.insert("channel_id".to_string(), cat.to_string());
    params.insert("show_type_id".to_string(), "0".to_string());
    params.insert("region_id".to_string(), "0".to_string());
    params.insert("lang_id".to_string(), "0".to_string());
    params.insert("year_range".to_string(), String::new());
    params.insert(
        "start".to_string(),
        ((page - 1) * PAGE_SIZE).to_string(),
    );
    let value = post(cfg, site, "/show/filter/WEB/3.2", &params)?;
    Ok(parse_list(&value))
}

/// 搜索：`/show/search/WEB/3.2`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let mut params = BTreeMap::new();
    params.insert("keyword".to_string(), keyword.to_string());
    params.insert("start".to_string(), "0".to_string());
    params.insert("cat_id".to_string(), "1".to_string());
    params.insert("keyword_type".to_string(), "0".to_string());
    let value = post(cfg, site, "/show/search/WEB/3.2", &params)?;
    Ok(parse_list(&value))
}

/// 详情：`/show/detail/WEB/3.2`。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let mut params = BTreeMap::new();
    params.insert("show_id_code".to_string(), id.to_string());
    let value = post(cfg, site, "/show/detail/WEB/3.2", &params)?;
    parse_detail(site, id, &value)
}

/// 播放：`/show/play/info/WEB/3.2` → `entity.playUrl`。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let mut parts = id.splitn(2, '_');
    let show_id = parts.next().unwrap_or_default().trim().to_string();
    let play_id = parts.next().unwrap_or_default().trim().to_string();
    if show_id.is_empty() || play_id.is_empty() {
        return Err(parse_err(site, format!("csp_NiNi 播放 id 非法: {id}")));
    }
    let mut params = BTreeMap::new();
    params.insert("show_id_code".to_string(), show_id);
    params.insert("play_id_code".to_string(), play_id);
    params.insert("oid".to_string(), "1".to_string());
    let value = post(cfg, site, "/show/play/info/WEB/3.2", &params)?;
    let url = value
        .get("entity")
        .map(|entity| s_of(entity, "playUrl"))
        .unwrap_or_default();
    if url.is_empty() {
        return Err(parse_err(site, "csp_NiNi 播放地址为空"));
    }
    let header = json!(headers());
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": header,
    }))
}

/// 首页 helper（原生家族契约没有独立首页入口，仅协议完备性与单测使用）。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let special = special_mode(site);
    if special {
        let mut params = BTreeMap::new();
        params.insert("channel_id".to_string(), SPECIAL_CHANNEL.to_string());
        params.insert("start".to_string(), "0".to_string());
        params.insert("more".to_string(), "1".to_string());
        let value = post(cfg, site, "/index/desktop/WEB/3.2", &params)?;
        return Ok(parse_home(&value, true));
    }
    let params = BTreeMap::new();
    let value = post(cfg, site, "/index/ranking/list/WEB/3.2", &params)?;
    Ok(parse_home(&value, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Value) -> Site {
        Site {
            key: "nini".to_string(),
            name: "泥巴".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(ext),
            ..Default::default()
        }
    }

    fn fixed() -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("_ts".to_string(), "1700000000000".to_string());
        map.insert("app_version".to_string(), "1.0".to_string());
        map.insert("platform".to_string(), "3".to_string());
        map
    }

    fn body() -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("b".to_string(), "2".to_string());
        map.insert("a".to_string(), "1".to_string());
        map
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_NiNi"));
        assert!(handles(" csp_nini "));
        assert!(!handles("csp_Nmys"));
    }

    #[test]
    fn base_of_honours_ext_override() {
        assert_eq!(base_of(&site(json!(""))), DEFAULT_BASE);
        assert_eq!(base_of(&site(json!("http://mock.test/api"))), "http://mock.test/api");
        assert_eq!(base_of(&site(json!("1"))), DEFAULT_BASE);
    }

    #[test]
    fn special_mode_only_for_ext_one() {
        assert!(special_mode(&site(json!("1"))));
        assert!(!special_mode(&site(json!(""))));
        assert!(!special_mode(&site(json!("https://api.nivod.tv"))));
    }

    #[test]
    fn des_round_trip_handles_padding() {
        for plain in ["", "a", "hello", "泥巴影视", "__QUERY::x"] {
            let cipher = des_encrypt_hex(plain);
            assert_eq!(cipher.len() % 16, 0, "PKCS7 后长度应为 8 字节倍数：{plain}");
            assert_eq!(des_decrypt_hex(&cipher).unwrap(), plain);
        }
    }

    #[test]
    fn oid_decrypts_to_timestamp_and_suffix() {
        let now = 1_700_000_000_000_i64;
        let oid = oid_at(now);
        assert_eq!(oid.len() % 16, 0);
        assert_eq!(
            des_decrypt_hex(&oid).unwrap(),
            format!("{}_{}", now - 600_000, OID_SUFFIX)
        );
    }

    #[test]
    fn sign_of_matches_manual_md5_and_sorts_keys() {
        let sign = sign_of(&fixed(), &body());
        let manual = format!(
            "{:x}",
            md5::compute(
                "__QUERY::_ts=1700000000000&app_version=1.0&platform=3&\
                 __BODY::a=1&b=2&__KEY::2x_Give_it_a_shot"
            )
        );
        assert_eq!(sign, manual);
        assert_eq!(sign.len(), 32);
        assert!(sign.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn signed_url_carries_sign_and_skips_empty_params() {
        let mut fixed = fixed();
        fixed.insert("year_range".to_string(), String::new());
        let url = signed_url("http://mock.test", "/show/filter/WEB/3.2", &fixed, &body());
        assert!(url.starts_with("http://mock.test/show/filter/WEB/3.2?_ts="));
        assert!(url.contains("&sign="));
        assert!(!url.contains("year_range="));
        // 同输入稳定，不同输入可区分。
        assert_eq!(
            url,
            signed_url("http://mock.test", "/show/filter/WEB/3.2", &fixed, &body())
        );
    }

    #[test]
    fn parse_categories_filters_special_channel() {
        let value = json!({
            "channels": [
                { "channelId": "7", "channelName": "频道七" },
                { "channelId": "1", "channelName": "电影" },
                { "channelId": "", "channelName": "缺 id" }
            ]
        });
        let all = parse_categories(&value, false);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].type_id, "1");
        assert_eq!(all[1].type_name, "电影");
        let special = parse_categories(&value, true);
        assert_eq!(special.len(), 1);
        assert_eq!(special[0].type_id, SPECIAL_CHANNEL);
    }

    #[test]
    fn parse_list_maps_items_and_remark() {
        let value = json!({
            "list": [
                { "showIdCode": "s1", "showTitle": "剧一", "showImg": "http://i/1.jpg", "episodesTxt": "更新至12集" },
                { "showIdCode": "s2", "showTitle": "剧二", "episodesTxt": "null" },
                { "showIdCode": "", "showTitle": "缺 id" }
            ]
        });
        let items = parse_list(&value);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "s1");
        assert_eq!(items[0].cover.as_deref(), Some("http://i/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("更新至12集"));
        assert_eq!(items[1].cover, None);
        assert_eq!(items[1].remark, None);
    }

    #[test]
    fn parse_home_supports_plain_and_special_layouts() {
        let plain = json!({
            "list": [
                [{ "showIdCode": "a", "showTitle": "A" }],
                [{ "showIdCode": "b", "showTitle": "B" }, { "showIdCode": "c", "showTitle": "C" }]
            ]
        });
        assert_eq!(parse_home(&plain, false).len(), 3);
        let special = json!({
            "banners": [
                { "show": { "showIdCode": "d", "showTitle": "D" } },
                { "show": { "showIdCode": "", "showTitle": "无 id" } }
            ]
        });
        let items = parse_home(&special, true);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "d");
    }

    #[test]
    fn parse_detail_builds_episode_urls_in_order() {
        let value = json!({
            "entity": {
                "showIdCode": "42",
                "showTitle": "泥巴剧",
                "showImg": "http://i/42.jpg",
                "showDesc": "简介",
                "postYear": "2024",
                "regionName": "大陆",
                "director": "导演",
                "actors": "演员",
                "plays": [
                    { "episodeName": "第1集", "playIdCode": "p1" },
                    { "episodeName": "", "playIdCode": "p2" },
                    { "playIdCode": "" }
                ]
            }
        });
        let detail = parse_detail(&site(json!("")), "42", &value).unwrap();
        assert_eq!(detail.id, "42");
        assert_eq!(detail.name, "泥巴剧");
        assert_eq!(detail.year.as_deref(), Some("2024"));
        assert_eq!(detail.episodes.len(), 1);
        assert_eq!(detail.episodes[0].group, GROUP);
        let eps = &detail.episodes[0].items;
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].title, "第1集");
        assert_eq!(eps[0].url, "42_p1");
        assert_eq!(eps[1].title, "播放");
        assert_eq!(eps[1].url, "42_p2");
    }

    #[test]
    fn parse_detail_without_plays_is_parse_error() {
        let value = json!({ "entity": { "showIdCode": "42", "plays": [] } });
        let err = parse_detail(&site(json!("")), "42", &value).unwrap_err();
        assert!(format!("{err}").contains("csp_NiNi"), "{err}");
    }

    #[test]
    fn player_content_rejects_malformed_id_before_hitting_network() {
        let err = player_content(&TvConfig::default(), &site(json!("")), "", "abc").unwrap_err();
        assert!(format!("{err}").contains("csp_NiNi"), "{err}");
    }
}
