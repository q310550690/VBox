//! `csp_Bdys01`（哔滴影视 bdys01 系）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * 类：`com.github.catvod.spider.Bdys01`，站点 1 个（GYCK 仓 `Bdys_spider`，
//!   站点 ext `./lib/token.json$$$https://www.bdys03.com/$$$null$$$1`）。
//! * 证据：`_rea/tmp/r141/Bdys01.bad.java`（jadx `--show-bad-code`，取自
//!   `_rea/samples/library/jars/a4d0f912-资源包1.jar`，514 行、0 skip）；
//!   对照变体 `_rea/tmp/r141/Bdys01.v2.java`（0f047745 版，837 行；`C0338a`
//!   异或串 202 处已整体解码，见 `_rea/tmp/r141/bdys-strings.json`）、
//!   `_rea/tmp/r141/Bdys01.v3.java`（dffec63f 版，1019 行，`extends Push`）。
//!   本家族取 a4d0f912 版语义（自包含 `spider.merge` 内的 jsoup/okhttp 子集，
//!   没有 `parser.merge` 宿主依赖）；两处上游差异见「有意偏差」。
//!
//! 协议（逐段还原）：
//! * 页面基址硬编码 `https://www.bdys01.com`（a4d0f912 版）；v2 版基址是
//!   `https://www.bdys10.com/` 并由 ext 首段覆盖。
//! * `homeContent`：分类固定三项 `全部=a / 电影=0 / 电视剧=1`；首页卡片
//!   `div.row.row-cards` 首块里的 `div.col-4.rows-md-7`（标题 `h3.card-title`、
//!   封面 `img.w-100[data-src]`、备注 `p.text-muted`、链接 `a.d-block.cover`）；
//!   结尾调用 `ot()`：GET `/zzzzz` 取 `set-cookie` 首段作为后续请求 Cookie。
//! * `categoryContent`：无筛选 `{base}/s/all/{page}?&type={type}&order=0`
//!   （`type=="a"` 时省略 `type`）；带筛选 `{base}/s/{s}/{page}?…` 模板替换
//!   `{s}/{area}/{year}/{order}` 后清理残留占位符；列表行 `div.col-lg-8`
//!   （标题 `h3.mb-0`、封面 `img.w-100[src]`、备注 `p.mb-0`、链接 `a.d-block[href]`）；
//!   页数取 `a.page-link` 中「尾页」链接里的数字，缺失时退化为当前页；
//!   空结果页含「没有找到您想要的结果哦」；`limit=48`。
//! * `detailContent`：`{base}{id}`；封面 `div.col-md-auto img[src]`、标题
//!   `h1.d-none.d-md-block`、简介 `div.card.collapse > div.card-body`、更新年份
//!   `span.badge.bg-purple-lt`（去 `更新时间：` 后取 `-` 前段）；信息行
//!   `div.col.mb-2` 的第一个块里的 `p`：`strong` 文本为标签——
//!   `类型：` 汇总其 `a` 文本、含 `地区` 的取 `[...]` 内文、含 `豆瓣` 的作备注、
//!   `导演：` 取首个 `a`、`主演：` 汇总 `a`；播放列表 `a.btn.btn-square`
//!   （文本`$`链接，链接含 `JSESSIONID` 时截断到 `;` 前），线路名「播放列表」。
//! * `playerContent`：播放页 `{base}{id}` 的 `script` 中找 `var pid =`，
//!   截到 `var time` 前，再取 `=` 之后、最后一个 `;` 之前为 pid；
//!   签名 `sg = AES-ECB-PKCS7(pid-ts, key=md5(pid-ts)[..16])` 十六进制大写，
//!   GET `{base}/lines?t={ts}&sg={sg}&pid={pid}`；`data` 里 `url3`/`m3u8`/
//!   `m3u8_2`（逗号分隔）汇总候选（`www.bde4.cc` 替换为页面域）；含 `mp4`
//!   直链返回 `{parse:0,url}`；否则 GET 候选取跳转后的最终地址、再取字节，
//!   跳过前 3354 字节后 GZIP 解压为 m3u8 文本，相对 `.ts` 行补
//!   `https://vod.bdys.me/` 前缀，最后以
//!   `data:application/vnd.apple.mpegurl;base64,…` 返回。
//! * `searchContent`：GET Google `site:www.bdys01.com {kw}`，取
//!   `div.yuRUbf a` 第一条（过滤 `/s/`、`play`、`performer`、`search`、
//!   `jsessionid`，且 `h3.LC20lb.MBeuO.DKV0Md` 文本须含关键词），再抓详情页
//!   取 `div.col-md-auto img[src]` 与 `h2.d-sm-block.d-md-none`。
//!
//! 有意偏差（桌面端能力边界 / 上游变体差异）：
//! 1. 上游当前不可匿名访问（`bdys01.com` 返回 JS 挑战壳、`bdys03.com` 404），
//!    本家族只做离线 fixture + mock 验收；`ext` 里第一个 `http(s)` 段可覆盖基址
//!    （仅测试/镜像钩子，a4d0f912 版原类没有该行为，v2 版有）。
//! 2. 原类 `playerContent` 从候选里 `Random` 取一条，本实现取第一条非空（可测）。
//! 3. 原类 `Vf.p` 只取响应头里的 `Location`，无跳转时 `Pd==null` 会抛异常并返回
//!    空串；本实现无 `Location` 时直接用候选地址继续（记偏差）。
//! 4. v2/v3 变体存在 `ext` 四段协议（`token.json$$$base$$$null$$$1`）与 `Push`
//!    基类语义；本家族按 a4d0f912 单段基址实现，忽略 token/弹幕段。
//! 5. 分类筛选（`filter`）在当前核心模型里没有 UI 承载，`videos` 收到空筛选时
//!    走无筛选 URL 分支（原类同路径）。

use std::collections::HashMap;
use std::sync::OnceLock;

use base64::Engine as _;
use scraper::Selector;
use serde_json::{json, Value};

use crate::error::Result;
use crate::http::client_for;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 站点 api 名。
pub const API: &str = "csp_Bdys01";

/// a4d0f912 版硬编码基址（无尾斜杠，原类所有 URL 为 `"https://www.bdys01.com" + path`）。
const DEFAULT_BASE: &str = "https://www.bdys01.com";

/// 原类 `iC()` 固定 UA。
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.114 Safari/537.36";

/// 原类 `iC()` 固定 Accept-Language。
const ACCEPT_LANG: &str = "zh-CN,zh;q=0.8,zh-TW;q=0.7,zh-HK;q=0.5,en-US;q=0.3,en;q=0.2";

/// m3u8 切片域名（原类 `.ts` 行补此前缀）。
const TS_HOST: &str = "https://vod.bdys.me/";

/// 原类跳过的二进制头长度（`Arrays.copyOfRange(byteArray, 3354, …)`）。
const GZIP_SKIP: usize = 3354;

/// 原类固定分页大小（核心模型不暴露 pagecount，仅作文档常量）。
#[allow(dead_code)]
const PAGE_LIMIT: u64 = 48;

/// 首页固定分类（原类 `homeContent` 的 JSON：`{"全部":"a","电影":"0","电视剧":"1"}`）。
const HOME_CATEGORIES: [(&str, &str); 3] = [("全部", "a"), ("电影", "0"), ("电视剧", "1")];

/// 空结果提示（原类 `h.contains(...)`）。
const EMPTY_HINT: &str = "没有找到您想要的结果哦";

/* ============================================================
 * 基础工具
 * ============================================================ */

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

macro_rules! selector {
    ($name:ident, $css:expr) => {
        fn $name() -> &'static Selector {
            static CACHE: OnceLock<Selector> = OnceLock::new();
            CACHE.get_or_init(|| Selector::parse($css).expect(concat!("非法选择器: ", $css)))
        }
    };
}

selector!(sel_page_link, "a.page-link");
selector!(sel_list_row, "div.col-lg-8");
selector!(sel_list_name, "h3.mb-0");
selector!(sel_img, "img.w-100");
selector!(sel_list_remark, "p.mb-0");
selector!(sel_list_link, "a.d-block");
selector!(sel_home_cards, "div.row.row-cards");
selector!(sel_home_row, "div.col-4.rows-md-7");
selector!(sel_home_name, "h3.card-title");
selector!(sel_home_remark, "p.text-muted");
selector!(sel_home_link, "a.d-block.cover");
selector!(sel_dt_cover, "div.col-md-auto img");
selector!(sel_dt_name, "h1.d-none.d-md-block");
selector!(sel_dt_desc, "div.card.collapse > div.card-body");
selector!(sel_dt_badge, "span.badge.bg-purple-lt");
selector!(sel_dt_info, "div.col.mb-2");
selector!(sel_dt_info_row, "p");
selector!(sel_dt_play, "a.btn.btn-square");
selector!(sel_search_row, "div.yuRUbf a");
selector!(sel_search_name, "h3.LC20lb.MBeuO.DKV0Md");
selector!(sel_search_detail_name, "h2.d-sm-block.d-md-none");
selector!(sel_strong, "strong");
selector!(sel_anchor, "a");

fn text_of(el: scraper::ElementRef<'_>, sel: &Selector) -> String {
    el.select(sel).next().map(|e| e.text().collect::<String>()).unwrap_or_default().trim().to_string()
}

fn attr_of(el: scraper::ElementRef<'_>, sel: &Selector, attr: &str) -> String {
    el.select(sel).next().and_then(|e| e.value().attr(attr)).unwrap_or_default().trim().to_string()
}

fn parse_html(html: &str) -> scraper::Html {
    scraper::Html::parse_document(html)
}

/// 基址：ext 里第一个 `http(s)` 段覆盖（原类硬编码；v2 版才有 ext 覆盖）。
pub fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    for seg in ext.split("$$$") {
        let seg = seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

fn host_of(base: &str) -> String {
    base.split("://").nth(1).unwrap_or(base).split('/').next().unwrap_or_default().to_string()
}

/// 原类 `iC(str, referer)`：UA + Authority + 可选 Referer/Cookie + Accept-Language。
fn page_headers(base: &str, referer: &str, cookie: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".into(), UA.into());
    if referer != "google" {
        h.insert("Authority".into(), host_of(base));
        if !referer.is_empty() {
            h.insert("Referer".into(), referer.into());
        }
        if !cookie.is_empty() {
            h.insert("Cookie".into(), cookie.into());
        }
    }
    h.insert("Accept-Language".into(), ACCEPT_LANG.into());
    h
}

/// 原类 `c(url)`：播放请求头（Authority=目标 host，Origin=页面基址）。
fn play_headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".into(), UA.into());
    h.insert("Authority".into(), host_of(base));
    h.insert("Origin".into(), base.to_string());
    h.insert("Accept-Language".into(), ACCEPT_LANG.into());
    h
}

fn get_bytes(cfg: &TvConfig, url: &str, headers: &HashMap<String, String>) -> Result<(Vec<u8>, HashMap<String, String>)> {
    let client = client_for(&cfg.http)?;
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send()?;
    let mut out = HashMap::new();
    for (k, v) in resp.headers().iter() {
        out.entry(k.as_str().to_ascii_lowercase())
            .or_insert_with(|| v.to_str().unwrap_or_default().to_string());
    }
    let bytes = resp
        .bytes()
        ?
        .to_vec();
    Ok((bytes, out))
}

fn get_text(cfg: &TvConfig, url: &str, headers: &HashMap<String, String>) -> Result<(String, HashMap<String, String>)> {
    let (bytes, h) = get_bytes(cfg, url, headers)?;
    Ok((String::from_utf8_lossy(&bytes).to_string(), h))
}

/// 原类 `ot()`：GET `/zzzzz` 取 `set-cookie` 首段（失败返回空串，不阻断）。
fn fetch_cookie(cfg: &TvConfig, base: &str) -> String {
    let url = format!("{base}/zzzzz");
    let headers = page_headers(base, base, "");
    match get_bytes(cfg, &url, &headers) {
        Ok((_, h)) => h
            .get("set-cookie")
            .map(|v| v.split(';').next().unwrap_or_default().to_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// 原类 `XT.w`：MD5 十六进制小写。
fn md5_hex(input: &str) -> String {
    format!("{:x}", md5::compute(input.as_bytes()))
}

/// 原类 `X(str,key)`：AES/ECB/PKCS7 加密后转十六进制大写。
pub fn aes_hex_upper(plain: &str, key: &str) -> Option<String> {
    use aes::Aes128;
    use ecb::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};
    let cipher = ecb::Encryptor::<Aes128>::new_from_slice(key.as_bytes()).ok()?;
    let raw = cipher.encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
    Some(raw.iter().map(|b| format!("{b:02X}")).collect())
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 原类分类页 URL（无筛选分支）。
pub fn list_url(base: &str, type_id: &str, page: u32) -> String {
    if type_id == "a" {
        format!("{base}/s/all/{page}?&order=0")
    } else {
        format!("{base}/s/all/{page}?&type={type_id}&order=0")
    }
}

/// 原类分类页 URL（带筛选分支；`filter` 为空时等价于无筛选）。
pub fn list_url_filtered(base: &str, type_id: &str, page: u32, filter: &HashMap<String, String>) -> String {
    if filter.is_empty() {
        return list_url(base, type_id, page);
    }
    let mut url = if type_id == "a" {
        format!("{base}/s/{{s}}/{page}?&area={{area}}&year={{year}}&order={{order}}")
    } else {
        format!("{base}/s/{{s}}/{page}?&type={type_id}&area={{area}}&year={{year}}&order={{order}}")
    };
    for (k, v) in filter {
        if v.is_empty() {
            continue;
        }
        url = url.replace(&format!("{{{k}}}"), &urlencode(v));
    }
    for _ in 0..4 {
        if url.contains("{s}") {
            url = url.replace("{s}", "all");
        } else if url.contains("{area}") {
            url = url.replace("&area={area}", "");
        } else if url.contains("{year}") {
            url = url.replace("&year={year}", "");
        } else if url.contains("{order}") {
            url = url.replace("{order}", "0");
        }
    }
    url
}

fn cleanup_id(mut id: String) -> String {
    if id.contains("JSESSIONID") {
        id = id.split(';').next().unwrap_or_default().to_string();
    }
    id
}

/// 页数：`a.page-link` 中「尾页」链接里的数字；缺失时退化为当前页。
pub fn parse_pagecount(html: &str, page: u32) -> u32 {
    let doc = parse_html(html);
    for a in doc.select(sel_page_link()) {
        let text = a.text().collect::<String>();
        if text.trim() != "尾页" {
            continue;
        }
        let href = a.value().attr("href").unwrap_or_default();
        let tail = if href.contains("JSESSIONID") {
            let idx = href.rfind('/').map(|i| i + 1).unwrap_or(0);
            let end = href.find(';').unwrap_or(href.len());
            href.get(idx..end).unwrap_or_default()
        } else {
            let idx = href.rfind('/').map(|i| i + 1).unwrap_or(0);
            let end = href.find('?').unwrap_or(href.len());
            href.get(idx..end).unwrap_or_default()
        };
        if let Ok(n) = tail.trim().parse::<u32>() {
            return n;
        }
    }
    page.max(1)
}

/// 原类列表页解析（`div.col-lg-8`）。
pub fn parse_list(html: &str) -> Vec<VideoItem> {
    if html.contains(EMPTY_HINT) {
        return Vec::new();
    }
    let doc = parse_html(html);
    let mut out = Vec::new();
    for row in doc.select(sel_list_row()) {
        let name = text_of(row, sel_list_name());
        let pic = attr_of(row, sel_img(), "src");
        let remark = text_of(row, sel_list_remark());
        let id = cleanup_id(attr_of(row, sel_list_link(), "href"));
        if name.is_empty() || id.is_empty() {
            continue;
        }
        out.push(VideoItem {
            id: id.clone(),
            name: name.clone(),
            cover: if pic.is_empty() { None } else { Some(pic) },
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

/// 原类首页卡片解析（`div.row.row-cards` 首块里的 `div.col-4.rows-md-7`）。
pub fn parse_home(html: &str) -> Vec<VideoItem> {
    let doc = parse_html(html);
    let Some(cards) = doc.select(sel_home_cards()).next() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in cards.select(sel_home_row()) {
        let name = text_of(row, sel_home_name());
        let pic = attr_of(row, sel_img(), "data-src");
        let remark = text_of(row, sel_home_remark());
        let id = cleanup_id(attr_of(row, sel_home_link(), "href"));
        if name.is_empty() || id.is_empty() {
            continue;
        }
        out.push(VideoItem {
            id: id.clone(),
            name,
            cover: if pic.is_empty() { None } else { Some(pic) },
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

/// 原类详情页元信息解析结果。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DetailMeta {
    pub name: String,
    pub cover: String,
    pub desc: String,
    pub year: String,
    pub area: String,
    pub remark: String,
    pub actor: String,
    pub director: String,
    pub type_name: String,
}

/// 原类详情页解析（选择器与标签切分逐段还原）。
pub fn parse_detail_page(html: &str) -> DetailMeta {
    let doc = parse_html(html);
    let mut meta = DetailMeta {
        name: text_of(doc.root_element(), sel_dt_name()),
        cover: attr_of(doc.root_element(), sel_dt_cover(), "src"),
        desc: text_of(doc.root_element(), sel_dt_desc()),
        ..Default::default()
    };
    let badge = text_of(doc.root_element(), sel_dt_badge());
    meta.year = badge
        .replace("更新时间：", "")
        .split('-')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if let Some(info) = doc.select(sel_dt_info()).next() {
        for p in info.select(sel_dt_info_row()) {
            let label = text_of(p, sel_strong());
            if label == "类型：" {
                let names: Vec<String> = p.select(sel_anchor())
                    .map(|a| a.text().collect::<String>().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                meta.type_name = format!("类型：{}", names.join(","));
            } else if label.contains("地区") {
                let text = p.text().collect::<String>();
                if let (Some(a), Some(b)) = (text.find('['), text.find(']')) {
                    meta.area = text[a + 1..b].trim().to_string();
                }
            } else if label.contains("豆瓣") {
                meta.remark = format!("豆瓣:{}", p.text().collect::<String>().trim());
            } else if label.contains("导演：") {
                meta.director = p
                    .select(sel_anchor())
                    .next()
                    .map(|a| a.text().collect::<String>().trim().to_string())
                    .unwrap_or_default();
            } else if label.contains("主演：") {
                let names: Vec<String> = p.select(sel_anchor())
                    .map(|a| a.text().collect::<String>().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                meta.actor = names.join(",");
            }
        }
    }
    meta
}

/// 原类播放列表解析（`a.btn.btn-square`）。
pub fn parse_playlist(html: &str) -> Vec<(String, String)> {
    let doc = parse_html(html);
    doc.select(sel_dt_play())
        .map(|a| {
            let name = a.text().collect::<String>().trim().to_string();
            let href = cleanup_id(a.value().attr("href").unwrap_or_default().to_string());
            (name, href)
        })
        .filter(|(name, href)| !href.is_empty() && !name.is_empty())
        .collect()
}

/* ============================================================
 * 时间 / id / gzip
 * ============================================================ */

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 从播放页 script 里取 `var pid = ...`（截到 `var time` 前，再取最后一个 `;` 之前）。
pub fn extract_pid(html: &str) -> Option<String> {
    let start = html.find("var pid")?;
    let rest = &html[start..];
    let end = rest.find("var time").map(|i| start + i).unwrap_or(html.len());
    let seg = &html[start..end];
    let eq = seg.find('=')?;
    let tail = seg[eq + 1..].trim_start();
    let semi = tail.rfind(';').unwrap_or(tail.len());
    let raw = tail[..semi].trim();
    let val = raw.trim_matches(|c: char| c == '\'' || c == '"' || c == ' ').trim();
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

/// 详情 id 归一：允许列表返回绝对 URL，统一折算成可拼到 base 后面的路径。
pub fn id_of(base: &str, href: &str) -> String {
    let h = href.trim();
    if h.is_empty() {
        return String::new();
    }
    if let Some(rest) = h.strip_prefix(base) {
        return rest.to_string();
    }
    if h.starts_with("http://") || h.starts_with("https://") {
        if let Ok(u) = url::Url::parse(h) {
            return u.path().to_string();
        }
    }
    h.to_string()
}

/// GZIP 解压（原类 `pS.w`）。
fn gunzip(bytes: &[u8]) -> Option<String> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut out = String::new();
    GzDecoder::new(bytes).read_to_string(&mut out).ok().map(|_| out)
}

/// 把 m3u8 文本里的相对 `.ts` 行补上固定切片域名（原类 `contains(".ts")` 分支）。
fn prefix_ts_lines(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let t = line.trim_end_matches('\r');
            if t.contains(".ts") && !t.trim_start().starts_with("http") && !t.trim_start().starts_with('#') {
                format!("{}{}", TS_HOST, t.trim_start())
            } else {
                t.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// data URI 输出（原类 `"data:application/vnd.apple.mpegurl;base64," + Base64.encode`）。
fn as_data_uri(text: &str) -> String {
    use base64::engine::general_purpose::STANDARD;
    format!(
        "data:application/vnd.apple.mpegurl;base64,{}",
        STANDARD.encode(text.as_bytes())
    )
}

/// 从 `/lines` 的 `data` 里汇总候选地址（`url3`/`m3u8`/`m3u8_2`）。
pub fn candidate_urls(base: &str, data: &Value) -> Vec<String> {
    let host = host_of(base);
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let v = raw.trim();
        if v.is_empty() {
            return;
        }
        let fixed = if v.contains("www.bde4.cc") {
            v.replace("www.bde4.cc", &host)
        } else {
            v.to_string()
        };
        if !out.contains(&fixed) {
            out.push(fixed);
        }
    };
    if let Some(v) = data.get("url3").and_then(|v| v.as_str()) {
        push(v);
    }
    if let Some(v) = data.get("m3u8").and_then(|v| v.as_str()) {
        push(v);
    }
    if let Some(v) = data.get("m3u8_2").and_then(|v| v.as_str()) {
        for part in v.split(',') {
            push(part);
        }
    }
    out
}

/* ============================================================
 * 五入口
 * ============================================================ */

/// 分类：原类 `homeContent` 固定三项。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(HOME_CATEGORIES
        .iter()
        .map(|(name, id)| Category {
            type_id: id.to_string(),
            type_name: name.to_string(),
        })
        .collect())
}

/// 分类列表。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
    filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url_filtered(&base, type_id, page, filter);
    let cookie = fetch_cookie(cfg, &base);
    let (html, _) = get_text(cfg, &url, &page_headers(&base, &url, &cookie))?;
    Ok(parse_list(&html)
        .into_iter()
        .map(|mut it| {
            it.id = id_of(&base, &it.id);
            it
        })
        .collect())
}

/// 详情。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let rid = id_of(&base, id);
    if rid.is_empty() {
        return Err(parse_err(site, "详情 id 为空"));
    }
    let url = format!("{base}{rid}");
    let cookie = fetch_cookie(cfg, &base);
    let (html, _) = get_text(cfg, &url, &page_headers(&base, &url, &cookie))?;
    let meta = parse_detail_page(&html);
    if meta.name.is_empty() {
        return Err(parse_err(site, format!("详情页缺少标题: {url}")));
    }
    let items: Vec<Episode> = parse_playlist(&html)
        .into_iter()
        .map(|(title, href)| Episode {
            title,
            url: id_of(&base, &href),
        })
        .filter(|e| !e.url.is_empty())
        .collect();
    if items.is_empty() {
        return Err(parse_err(site, format!("详情页缺少播放列表: {url}")));
    }
    let mut desc = meta.desc;
    if !meta.type_name.is_empty() {
        desc = format!("{}\n{}", meta.type_name, desc.trim()).trim().to_string();
    }
    Ok(VideoDetail {
        id: rid,
        name: meta.name,
        cover: if meta.cover.is_empty() { None } else { Some(meta.cover) },
        desc: if desc.is_empty() { None } else { Some(desc) },
        year: if meta.year.is_empty() { None } else { Some(meta.year) },
        area: if meta.area.is_empty() { None } else { Some(meta.area) },
        director: if meta.director.is_empty() { None } else { Some(meta.director) },
        actor: if meta.actor.is_empty() { None } else { Some(meta.actor) },
        episodes: vec![EpisodeGroup {
            group: "播放列表".to_string(),
            items,
        }],
        ..Default::default()
    })
}

/// 搜索：Google `site:` + 首个命中，再抓详情页取标题/封面（原类语义）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let kw = keyword.trim();
    if kw.is_empty() {
        return Ok(Vec::new());
    }
    let base = base_of(site);
    let google = format!(
        "https://www.google.com/search?q=site%3Awww.bdys01.com+{}&hl=zh-CN",
        urlencode(kw)
    );
    let headers = page_headers(&base, "google", "");
    let (html, _) = match get_text(cfg, &google, &headers) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    let Some(href) = first_search_hit(&html, kw) else {
        return Ok(Vec::new());
    };
    let detail_url = if href.starts_with("http") {
        href.clone()
    } else {
        format!("{base}{href}")
    };
    let cookie = fetch_cookie(cfg, &base);
    let (detail_html, _) = match get_text(cfg, &detail_url, &page_headers(&base, "google", &cookie)) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    let name = text_of(parse_html(&detail_html).root_element(), sel_search_detail_name());
    let name = if name.is_empty() {
        text_of(parse_html(&detail_html).root_element(), sel_dt_name())
    } else {
        name
    };
    if name.is_empty() {
        return Ok(Vec::new());
    }
    let cover = attr_of(parse_html(&detail_html).root_element(), sel_dt_cover(), "src");
    Ok(vec![VideoItem {
        id: id_of(&base, &href),
        name,
        cover: if cover.is_empty() { None } else { Some(cover) },
        remark: None,
    }])
}

/// 搜索命中的第一条（过滤非影视路径）。
pub fn first_search_hit(html: &str, keyword: &str) -> Option<String> {
    let doc = parse_html(html);
    for a in doc.select(sel_search_row()) {
        let href = a.value().attr("href").unwrap_or_default().trim().to_string();
        if href.is_empty() || href.starts_with("/search") {
            continue;
        }
        let low = href.to_ascii_lowercase();
        if low.contains("/s/") || low.contains("play") || low.contains("performer")
            || low.contains("search") || low.contains("jsessionid")
        {
            continue;
        }
        let title = a
            .select(sel_search_name())
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_else(|| a.text().collect::<String>());
        if !title.trim().is_empty() && title.contains(keyword) {
            return Some(href);
        }
    }
    None
}

/// 播放：pid 提取 → AES 签名 → `/lines` 候选 → 直链或 m3u8 密文（3354 跳过 + GZIP + ts 前缀 + data URI）。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let rid = id_of(&base, id);
    if rid.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    let page_url = format!("{base}{rid}");
    let cookie = fetch_cookie(cfg, &base);
    let (html, _) = get_text(cfg, &page_url, &page_headers(&base, &page_url, &cookie))?;
    let pid = extract_pid(&html).ok_or_else(|| parse_err(site, format!("播放页缺少 pid: {page_url}")))?;
    let ts = now_ms();
    let plain = format!("{pid}{ts}");
    let key = md5_hex(&plain);
    let key16 = &key[..16.min(key.len())];
    let sg = aes_hex_upper(&plain, key16)
        .ok_or_else(|| parse_err(site, "AES 签名失败：密钥长度不合法"))?;
    let lines_url = format!("{base}/lines?t={ts}&sg={sg}&pid={pid}");
    let (body, _) = get_text(cfg, &lines_url, &page_headers(&base, &page_url, &cookie))?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| parse_err(site, format!("lines JSON 非法: {e}")))?;
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    let candidates = candidate_urls(&base, &data);
    let Some(first) = candidates.first() else {
        return Err(parse_err(site, "lines 返回空候选"));
    };
    if candidates.iter().any(|u| u.contains("mp4")) {
        return Ok(json!({ "parse": 0, "playUrl": "", "url": first }));
    }
    for cand in &candidates {
        if let Some(url) = resolve_m3u8(cfg, &base, cand) {
            return Ok(json!({
                "parse": 0,
                "playUrl": "",
                "url": as_data_uri(&url),
                "header": play_headers(&base),
            }));
        }
    }
    Err(parse_err(site, "m3u8 密文解析失败（候选全部不可用）"))
}

/// 取 m3u8：跟 `Location` → 跳过 3354 字节 → GZIP → 补 `.ts` 前缀。
fn resolve_m3u8(cfg: &TvConfig, base: &str, candidate: &str) -> Option<String> {
    let headers = play_headers(base);
    let (_, resp_headers) = get_bytes(cfg, candidate, &headers).ok()?;
    let final_url = resp_headers
        .get("location")
        .cloned()
        .unwrap_or_else(|| candidate.to_string());
    let (bytes, _) = if final_url == candidate {
        get_bytes(cfg, candidate, &headers).ok()?
    } else {
        get_bytes(cfg, &final_url, &headers).ok()?
    };
    if bytes.len() <= GZIP_SKIP {
        return None;
    }
    let text = gunzip(&bytes[GZIP_SKIP..])?;
    let prefixed = prefix_ts_lines(&text);
    if prefixed.trim().is_empty() {
        None
    } else {
        Some(prefixed)
    }
}

/* ============================================================
 * 单测（纯函数，离线）
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site_with_ext(ext: &str) -> Site {
        Site {
            key: "bdys01".into(),
            name: "哔滴".into(),
            api: API.into(),
            ext: Some(serde_json::Value::String(ext.into())),
            ..Default::default()
        }
    }

    #[test]
    fn handles_api_case_insensitively() {
        assert!(handles("csp_Bdys01"));
        assert!(handles(" CSP_BDYS01 "));
        assert!(!handles("csp_Bdys010"));
    }

    #[test]
    fn base_of_defaults_and_ext_override() {
        assert_eq!(base_of(&site_with_ext("")), DEFAULT_BASE);
        assert_eq!(
            base_of(&site_with_ext("token.json$$$https://www.bdys03.com/$$$null$$$1")),
            "https://www.bdys03.com"
        );
    }

    #[test]
    fn page_headers_carry_authority_referer_cookie() {
        let h = page_headers("https://www.bdys01.com", "https://www.bdys01.com/mv/1.html", "w=abc");
        assert_eq!(h.get("Authority").map(String::as_str), Some("www.bdys01.com"));
        assert_eq!(h.get("Cookie").map(String::as_str), Some("w=abc"));
        assert!(h.get("Referer").is_some());
        let g = page_headers("https://www.bdys01.com", "google", "");
        assert!(g.get("Referer").is_none());
        assert!(g.get("Authority").is_none());
    }

    #[test]
    fn play_headers_use_origin() {
        let h = play_headers("https://www.bdys01.com");
        assert_eq!(h.get("Origin").map(String::as_str), Some("https://www.bdys01.com"));
        assert_eq!(h.get("Authority").map(String::as_str), Some("www.bdys01.com"));
    }

    #[test]
    fn list_url_branches() {
        assert_eq!(list_url("https://b", "a", 2), "https://b/s/all/2?&order=0");
        assert_eq!(list_url("https://b", "0", 2), "https://b/s/all/2?&type=0&order=0");
    }

    #[test]
    fn list_url_filtered_falls_back_and_replaces() {
        let empty = HashMap::new();
        assert_eq!(list_url_filtered("https://b", "a", 1, &empty), list_url("https://b", "a", 1));
        let mut f = HashMap::new();
        f.insert("area".to_string(), "日本".to_string());
        f.insert("year".to_string(), "2023".to_string());
        let url = list_url_filtered("https://b", "1", 3, &f);
        assert!(url.contains("/s/all/3"), "got {url}");
        assert!(url.contains("&type=1"), "got {url}");
        assert!(!url.contains("{area}") && !url.contains("{year}"), "got {url}");
    }

    #[test]
    fn cleanup_id_strips_jsessionid() {
        assert_eq!(cleanup_id("/mv/1.html;JSESSIONID=abc".into()), "/mv/1.html");
        assert_eq!(cleanup_id("/mv/1.html".into()), "/mv/1.html");
    }

    #[test]
    fn pagecount_reads_tail_link() {
        let html = r#"<a class="page-link" href="/s/all/7;JSESSIONID=x">尾页</a>"#;
        assert_eq!(parse_pagecount(html, 2), 7);
        assert_eq!(parse_pagecount("<html></html>", 3), 3);
    }

    #[test]
    fn parse_list_rows() {
        let html = r#"
        <div class="col-lg-8">
          <h3 class="mb-0">片一</h3>
          <img class="w-100" src="https://img/1.jpg">
          <p class="mb-0">更新至10集</p>
          <a class="d-block" href="/mv/11.html;JSESSIONID=x">播放</a>
        </div>"#;
        let rows = parse_list(html);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "片一");
        assert_eq!(rows[0].id, "/mv/11.html");
        assert_eq!(rows[0].cover.as_deref(), Some("https://img/1.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("更新至10集"));
        assert!(parse_list("没有找到您想要的结果哦").is_empty());
    }

    #[test]
    fn parse_home_cards() {
        let html = r#"
        <div class="row row-cards">
          <div class="col-4 rows-md-7">
            <h3 class="card-title">首页片</h3>
            <img class="w-100" data-src="https://img/h.jpg">
            <p class="text-muted">备注</p>
            <a class="d-block cover" href="/mv/9.html">看</a>
          </div>
        </div>"#;
        let rows = parse_home(html);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "/mv/9.html");
        assert_eq!(rows[0].cover.as_deref(), Some("https://img/h.jpg"));
    }

    #[test]
    fn parse_detail_meta_and_playlist() {
        let html = r#"
        <div class="col-md-auto"><img src="https://img/d.jpg"></div>
        <h1 class="d-none d-md-block">详情片名</h1>
        <div class="card collapse"><div class="card-body">剧情简介</div></div>
        <span class="badge bg-purple-lt">更新时间：2024-08-01</span>
        <div class="col mb-2">
          <p><strong>类型：</strong><a>电影</a></p>
          <p><strong>地区：</strong>[日本]</p>
          <p><strong>豆瓣：</strong>8.1</p>
          <p><strong>导演：</strong><a>某导演</a></p>
          <p><strong>主演：</strong><a>甲</a><a>乙</a></p>
        </div>
        <a class="btn btn-square" href="/play/1-1-1.html;JSESSIONID=z">第1集</a>
        <a class="btn btn-square" href="/play/1-1-2.html">第2集</a>"#;
        let meta = parse_detail_page(html);
        assert_eq!(meta.name, "详情片名");
        assert_eq!(meta.cover, "https://img/d.jpg");
        assert_eq!(meta.desc, "剧情简介");
        assert_eq!(meta.year, "2024");
        assert_eq!(meta.area, "日本");
        assert_eq!(meta.director, "某导演");
        assert_eq!(meta.actor, "甲,乙");
        assert!(meta.type_name.contains("电影"));
        let items = parse_playlist(html);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], ("第1集".to_string(), "/play/1-1-1.html".to_string()));
    }

    #[test]
    fn extract_pid_between_markers() {
        let html = "<script>var pid = 'abc123';\nvar time = 1; </script>";
        assert_eq!(extract_pid(html).as_deref(), Some("abc123"));
        assert!(extract_pid("<script></script>").is_none());
        let html2 = "<script>var pid='p9';var time=2;</script>";
        assert_eq!(extract_pid(html2).as_deref(), Some("p9"));
    }

    #[test]
    fn id_of_strips_base_and_host() {
        assert_eq!(id_of("https://www.bdys01.com", "https://www.bdys01.com/mv/1.html"), "/mv/1.html");
        assert_eq!(id_of("https://www.bdys01.com", "/mv/2.html"), "/mv/2.html");
        assert_eq!(id_of("https://www.bdys01.com", "https://other.com/p/3.html"), "/p/3.html");
    }

    #[test]
    fn candidate_urls_dedup_and_rewrite() {
        let data = json!({
            "url3": "https://www.bde4.cc/a.m3u8",
            "m3u8": "https://cdn/b.m3u8",
            "m3u8_2": "https://cdn/b.m3u8,https://cdn/c.m3u8"
        });
        let got = candidate_urls("https://www.bdys01.com", &data);
        assert_eq!(got.len(), 3);
        assert!(got[0].contains("www.bdys01.com"));
        assert!(got.contains(&"https://cdn/b.m3u8".to_string()));
    }

    #[test]
    fn aes_roundtrip_and_ts_prefix() {
        use aes::Aes128;
        use ecb::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};
        let plain = "pid1231700000000000";
        let key = md5_hex(plain);
        let key16 = &key[..16];
        let enc = aes_hex_upper(plain, key16).expect("encrypt");
        assert_eq!(enc.len(), 64);
        assert!(enc.chars().all(|c| c.is_ascii_hexdigit() && !c.is_lowercase()));
        let raw = hex::decode(enc.to_ascii_lowercase()).expect("hex");
        let dec = ecb::Decryptor::<Aes128>::new_from_slice(key16.as_bytes())
            .expect("key")
            .decrypt_padded_vec_mut::<Pkcs7>(&raw)
            .expect("decrypt");
        assert_eq!(String::from_utf8(dec).expect("utf8"), plain);
        assert_eq!(
            prefix_ts_lines("#EXTM3U\nseg1.ts\nhttps://x/y.ts\n"),
            "#EXTM3U\nhttps://vod.bdys.me/seg1.ts\nhttps://x/y.ts\n"
        );
    }
}
