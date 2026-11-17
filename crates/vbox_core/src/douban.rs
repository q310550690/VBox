//! 豆瓣首页数据（点播首页用）
//!
//! 数据来源是豆瓣移动端 h5 的 rexxar 接口：
//! - 集合：`/rexxar/api/v2/subject_collection/{id}/items`
//! - 详情：`/rexxar/api/v2/movie/{id}` 或 `/rexxar/api/v2/tv/{id}`
//!
//! 这两个接口必须带桌面 Chrome UA + `Referer: https://m.douban.com/`，
//! 否则会返回 418 / 403。别的模块都用 `crate::http`，这里需要专用请求头，
//! 所以单独构建一个阻塞客户端，并在 `std::thread::scope` 里并行拉取集合。
//!
//! 缓存分两级：进程内快照 + 可选磁盘文件（`home_with_cache` 传入路径时启用）。
//! 集合按「10 分钟 TTL」增量刷新：未过期的集合不再请求网络，失败的集合保留旧数据。
//! 启动时可以先读磁盘缓存直接渲染（`cached_home`），再后台调 `home_with_cache` 增量更新。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 集合接口；`{key}` 为 subject_collection id。
const COLLECTION_API: &str = "https://m.douban.com/rexxar/api/v2/subject_collection";
/// 条目详情接口；`{kind}` 为 movie / tv，`{id}` 为豆瓣条目 id。
const SUBJECT_API: &str = "https://m.douban.com/rexxar/api/v2";
/// 首页展示的集合（顺序即前端展示顺序）。
pub const COLLECTION_KEYS: [(&str, &str); 8] = [
    ("movie_hot_gaia", "热门电影"),
    ("tv_hot", "热门剧集"),
    ("show_hot", "热门综艺"),
    ("tv_animation", "热门动漫"),
    ("movie_top250", "TOP250"),
    ("movie_showing", "影院热映"),
    ("movie_soon", "即将上映"),
    ("tv_documentary", "纪录片"),
];

/// 轮播条数：热门电影前 5 + 热门剧集前 5。
const CAROUSEL_PER_COLLECTION: usize = 5;
/// 集合缓存有效期 10 分钟（毫秒）。
const CACHE_TTL_MS: u64 = 600_000;
/// 磁盘缓存版本；版本不符时按「无缓存」处理。
const CACHE_FILE_VERSION: u32 = 2;
/// 单个请求超时 12 秒。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                   (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
const REFERER: &str = "https://m.douban.com/";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DoubanItem {
    pub id: String,
    pub title: String,
    pub cover: Option<String>,
    pub rating: Option<f64>,
    pub year: Option<String>,
    /// 地区 / 类型 / 导演 / 主演
    pub info: Option<String>,
    /// 剧情简介（轮播项才有）
    pub intro: Option<String>,
    pub subtype: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoubanCollection {
    pub key: String,
    pub name: String,
    pub items: Vec<DoubanItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoubanHome {
    pub carousel: Vec<DoubanItem>,
    pub collections: Vec<DoubanCollection>,
    pub fetched_at: u64,
    pub cache_hit: bool,
    /// 由命令层注入（形如 `http://127.0.0.1:port/img?url=`）；核心层保持 None。
    pub image_proxy: Option<String>,
}

/* ---------------- 两级缓存结构 ---------------- */

/// 单个集合的缓存条目（内存 / 磁盘共用）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CachedCollection {
    key: String,
    name: String,
    /// 上次成功抓取的时间（毫秒时间戳）
    fetched_at: u64,
    items: Vec<DoubanItem>,
}

/// 轮播条目：记下来源集合与详情补全时间，增量刷新时可以直接复用。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CachedCarouselItem {
    /// 来源：movie（movie_hot_gaia）/ tv（tv_hot）
    source: String,
    /// 详情补全时间（毫秒时间戳）
    enriched_at: u64,
    item: DoubanItem,
}

/// 首页缓存快照；内存与磁盘（`<app_data_dir>/douban-home.json`）共用同一结构。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DiskCache {
    version: u32,
    saved_at: u64,
    collections: Vec<CachedCollection>,
    carousel: Vec<CachedCarouselItem>,
}

/// 进程内缓存：按集合保存 `fetchedAt`，便于增量刷新（未过期集合不再请求）。
static CACHE: Mutex<Option<DiskCache>> = Mutex::new(None);
/// 串行化整个 home 调用：避免并发重复抓取、以及磁盘写入互相覆盖。
static HOME_LOCK: Mutex<()> = Mutex::new(());

/// 抓取豆瓣首页；`force = true` 跳过缓存强制刷新。
///
/// 兼容旧调用（CLI / 测试）：不启用磁盘缓存，只用进程内缓存。
pub fn home(force: bool) -> Result<DoubanHome, String> {
    home_with_cache(None, force)
}

/// 抓取豆瓣首页（带可选磁盘缓存）。
///
/// - `cache_path`：磁盘缓存文件；`None` 时只使用进程内缓存。
/// - `force`：`true` 时忽略所有缓存，全量刷新。
///
/// 增量语义：只有过期（超过 10 分钟）或缺失的集合才会发网络请求；
/// 单个集合失败保留旧数据，只有全部集合都不可用时才返回错误。
pub fn home_with_cache(cache_path: Option<&Path>, force: bool) -> Result<DoubanHome, String> {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // 1. 取缓存快照：优先内存；没有且调用方给了路径时读磁盘。
    let mut cache = match snapshot_cache() {
        Some(cache) => cache,
        None => match cache_path {
            Some(path) => read_disk_cache(path).unwrap_or_default(),
            None => DiskCache::default(),
        },
    };

    let now = now_millis();

    // 2. 计算过期集合：force 时全部刷新；否则只刷新缺失 / 超过 TTL 的集合。
    let stale: Vec<(&'static str, &'static str)> = COLLECTION_KEYS
        .iter()
        .copied()
        .filter(|(key, _)| force || is_stale(&cache, key, now))
        .collect();

    let mut client: Option<reqwest::blocking::Client> = None;
    let mut first_error: Option<String> = None;
    let mut fetched_any = false;

    if !stale.is_empty() {
        match build_client() {
            Ok(built) => client = Some(built),
            Err(err) => {
                tracing::warn!("豆瓣 HTTP 客户端创建失败，沿用旧缓存: {err}");
                first_error = Some(err);
            }
        }
        if let Some(client) = client.as_ref() {
            // 3. 只抓过期集合；成功的更新 fetchedAt，失败的保留旧数据。
            for (key, name, result) in fetch_collections(client, &stale) {
                match result {
                    Ok(items) => {
                        fetched_any = true;
                        upsert_collection(&mut cache, key, name, items, now);
                    }
                    Err(err) => {
                        tracing::warn!("豆瓣集合 {key} 拉取失败，保留旧数据: {err}");
                        if first_error.is_none() {
                            first_error = Some(err);
                        }
                    }
                }
            }
        }
    }

    // 合并后仍然没有任何集合数据：返回明确的错误（前端展示重试入口）。
    if cache.collections.is_empty() {
        let detail = first_error.unwrap_or_else(|| "未知错误".to_string());
        return Err(format!(
            "豆瓣首页数据拉取失败：{} 个集合全部请求失败（{detail}）",
            COLLECTION_KEYS.len()
        ));
    }

    // 4. 轮播重建：强制刷新、来源集合被刷新、或缓存里还没有轮播时。
    let carousel_stale = force
        || cache.carousel.is_empty()
        || stale
            .iter()
            .any(|(key, _)| *key == "movie_hot_gaia" || *key == "tv_hot");
    if carousel_stale {
        if client.is_none() {
            match build_client() {
                Ok(built) => client = Some(built),
                Err(err) => tracing::warn!("豆瓣轮播详情补全不可用，沿用旧轮播: {err}"),
            }
        }
        if let Some(client) = client.as_ref() {
            let previous = cache.carousel.clone();
            let mut carousel = Vec::with_capacity(CAROUSEL_PER_COLLECTION * 2);
            for (mut item, kind) in seed_items(&cache.collections) {
                // 同 id 且已补全简介的旧条目直接复用（保留原 enrichedAt）。
                if let Some(reused) = carousel_reuse(&previous, &item, kind, force) {
                    carousel.push(reused);
                    continue;
                }
                enrich_item(client, &mut item, kind);
                carousel.push(CachedCarouselItem {
                    source: kind.to_string(),
                    enriched_at: now_millis(),
                    item,
                });
            }
            cache.carousel = carousel;
        }
    }

    // 5. 组装返回值（cacheHit = 本次没有成功抓到任何集合）。
    let home = build_home(&cache, !fetched_any);

    // 6. 写回内存缓存；有路径时原子写磁盘（失败只打日志，不阻断本次返回）。
    cache.version = CACHE_FILE_VERSION;
    cache.saved_at = now_millis();
    store_cache(&cache);
    if let Some(path) = cache_path {
        write_disk_cache(path, &cache);
    }

    Ok(home)
}

/// 只读磁盘缓存并组装首页（不联网、不碰内存缓存）。
///
/// 文件缺失 / 损坏 / 版本不符 / 没有任何集合数据时返回 `None`。
pub fn cached_home(cache_path: &Path) -> Option<DoubanHome> {
    let cache = read_disk_cache(cache_path)?;
    let home = build_home(&cache, true);
    if home.collections.is_empty() {
        return None;
    }
    Some(home)
}

/// 取内存缓存快照（短暂持锁后立即释放）。
fn snapshot_cache() -> Option<DiskCache> {
    CACHE.lock().ok().and_then(|guard| guard.as_ref().cloned())
}

/// 写回内存缓存快照。
fn store_cache(cache: &DiskCache) {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(cache.clone());
    }
}

/// 集合是否过期：缓存里没有，或距上次抓取已超过 TTL。
fn is_stale(cache: &DiskCache, key: &str, now: u64) -> bool {
    match cache.collections.iter().find(|entry| entry.key == key) {
        Some(entry) => now.saturating_sub(entry.fetched_at) >= CACHE_TTL_MS,
        None => true,
    }
}

/// 更新（或插入）单个集合缓存。
fn upsert_collection(
    cache: &mut DiskCache,
    key: &str,
    name: &str,
    items: Vec<DoubanItem>,
    now: u64,
) {
    match cache.collections.iter_mut().find(|entry| entry.key == key) {
        Some(entry) => {
            entry.name = name.to_string();
            entry.items = items;
            entry.fetched_at = now;
        }
        None => cache.collections.push(CachedCollection {
            key: key.to_string(),
            name: name.to_string(),
            fetched_at: now,
            items,
        }),
    }
}

/// 从集合缓存中提取轮播种子：热门电影前 5 + 热门剧集前 5。
fn seed_items(collections: &[CachedCollection]) -> Vec<(DoubanItem, &'static str)> {
    const SOURCES: [(&str, &str); 2] = [("movie_hot_gaia", "movie"), ("tv_hot", "tv")];
    let mut seeds = Vec::with_capacity(CAROUSEL_PER_COLLECTION * SOURCES.len());
    for (key, kind) in SOURCES {
        if let Some(collection) = collections.iter().find(|entry| entry.key == key) {
            seeds.extend(
                collection
                    .items
                    .iter()
                    .take(CAROUSEL_PER_COLLECTION)
                    .cloned()
                    .map(|item| (item, kind)),
            );
        }
    }
    seeds
}

/// 轮播复用判断：`force` 时一律不复用；同来源同 id 且已补全简介的旧条目直接复用。
fn carousel_reuse(
    cached: &[CachedCarouselItem],
    seed: &DoubanItem,
    source: &str,
    force: bool,
) -> Option<CachedCarouselItem> {
    if force {
        return None;
    }
    cached
        .iter()
        .find(|entry| {
            entry.source == source && entry.item.id == seed.id && entry.item.intro.is_some()
        })
        .cloned()
}

/// 用缓存快照组装 [`DoubanHome`]：集合按 [`COLLECTION_KEYS`] 顺序，`fetched_at` 取各集合最大值。
fn build_home(cache: &DiskCache, cache_hit: bool) -> DoubanHome {
    let mut collections = Vec::with_capacity(cache.collections.len());
    let mut fetched_at = 0u64;
    for (key, _) in COLLECTION_KEYS {
        let Some(entry) = cache.collections.iter().find(|entry| entry.key == key) else {
            continue;
        };
        if entry.items.is_empty() {
            continue;
        }
        fetched_at = fetched_at.max(entry.fetched_at);
        collections.push(DoubanCollection {
            key: entry.key.clone(),
            name: entry.name.clone(),
            items: entry.items.clone(),
        });
    }
    DoubanHome {
        carousel: cache
            .carousel
            .iter()
            .map(|entry| entry.item.clone())
            .collect(),
        collections,
        fetched_at,
        cache_hit,
        image_proxy: None,
    }
}

/// 读取磁盘缓存；文件缺失 / 损坏 / 版本不符时返回 `None`。
fn read_disk_cache(path: &Path) -> Option<DiskCache> {
    let text = std::fs::read_to_string(path).ok()?;
    let cache: DiskCache = serde_json::from_str(&text).ok()?;
    if cache.version != CACHE_FILE_VERSION {
        return None;
    }
    Some(cache)
}

/// 写磁盘缓存：失败只打日志，不影响本次返回。
fn write_disk_cache(path: &Path, cache: &DiskCache) {
    // 落盘前统一打上当前版本号（内存/磁盘结构一致，读回时据此判断兼容性）。
    let mut snapshot = cache.clone();
    snapshot.version = CACHE_FILE_VERSION;
    let data = match serde_json::to_string_pretty(&snapshot) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("[vbox] 豆瓣首页缓存序列化失败: {err}");
            return;
        }
    };
    if let Err(err) = write_atomic(path, &data) {
        eprintln!("[vbox] 豆瓣首页缓存写入失败({}): {err}", path.display());
    }
}

/// 原子写文件：先写临时文件再 rename；Windows 上 rename 失败时删目标重试一次。
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Windows 上目标被短暂占用时 rename 可能失败：删掉目标再试一次
            if std::fs::remove_file(path).is_ok() && std::fs::rename(&tmp, path).is_ok() {
                return Ok(());
            }
            let _ = std::fs::remove_file(&tmp);
            Err(first)
        }
    }
}

fn build_client() -> Result<reqwest::blocking::Client, String> {
    use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, REFERER as REFERER_HEADER, USER_AGENT};

    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(UA));
    headers.insert(REFERER_HEADER, HeaderValue::from_static(REFERER));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/plain, */*"),
    );

    reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .default_headers(headers)
        .build()
        .map_err(|e| format!("创建豆瓣 HTTP 客户端失败: {e}"))
}

/// 并行拉取指定集合（增量刷新只传过期集合）；返回顺序与传入顺序一致。
fn fetch_collections(
    client: &reqwest::blocking::Client,
    keys: &[(&'static str, &'static str)],
) -> Vec<(&'static str, &'static str, Result<Vec<DoubanItem>, String>)> {
    let mut rows: Vec<(&'static str, &'static str, Result<Vec<DoubanItem>, String>)> =
        Vec::with_capacity(keys.len());

    std::thread::scope(|scope| {
        let handles: Vec<_> = keys
            .iter()
            .enumerate()
            .map(|(index, (key, name))| {
                scope.spawn(move || {
                    let result = fetch_collection(client, key);
                    (index, *key, *name, result)
                })
            })
            .collect();

        let mut joined = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.join() {
                Ok(row) => joined.push(row),
                Err(_) => joined.push((
                    usize::MAX,
                    "unknown",
                    "未知集合",
                    Err("抓取线程异常退出".to_string()),
                )),
            }
        }
        joined.sort_by_key(|row| row.0);
        rows = joined
            .into_iter()
            .map(|(_, key, name, result)| (key, name, result))
            .collect();
    });

    rows
}

/// 拉取单个集合：网络型接口在并发批量拉取时偶发超时/限流，失败后重试一次。
///
/// 背景：真站审计以 --test-threads=4 并行跑 live 用例，豆瓣首页同时抓 8 个集合，
/// 上游偶发只返回部分集合（2026-11-14 实测一次 4 passed / 1 failed）。这里做
/// 单次重试（带 300ms 起、按 key 抖动到 700ms 的退避），把瞬时失败挡在数据层，
/// 而不是靠调用方重跑整个审计。重试仍失败时同时保留两次错误，便于定位。
fn fetch_collection(
    client: &reqwest::blocking::Client,
    key: &str,
) -> Result<Vec<DoubanItem>, String> {
    match fetch_collection_once(client, key) {
        Ok(items) => Ok(items),
        Err(first) => {
            let jitter = u64::from(key.bytes().map(u64::from).sum::<u64>() % 5) * 100;
            std::thread::sleep(std::time::Duration::from_millis(300 + jitter));
            fetch_collection_once(client, key)
                .map_err(|second| format!("{first}；重试失败：{second}"))
        }
    }
}

fn fetch_collection_once(
    client: &reqwest::blocking::Client,
    key: &str,
) -> Result<Vec<DoubanItem>, String> {
    let url = format!("{COLLECTION_API}/{key}/items?start=0&count=18");
    let value = get_json(client, &url)?;
    let items = value
        .get("subject_collection_items")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{key} 响应缺少 subject_collection_items"))?;

    let normalized: Vec<DoubanItem> = items.iter().filter_map(|raw| normalize_item(raw)).collect();

    if normalized.is_empty() {
        return Err(format!("{key} 返回的条目为空"));
    }
    Ok(normalized)
}

fn get_json(client: &reqwest::blocking::Client, url: &str) -> Result<Value, String> {
    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("请求豆瓣接口失败 {url}: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("豆瓣接口返回 HTTP {status}: {url}"));
    }

    let text = response
        .text()
        .map_err(|e| format!("读取豆瓣接口响应失败 {url}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析豆瓣接口 JSON 失败 {url}: {e}"))
}

/// 把集合条目（三种响应形状）归一化成 [`DoubanItem`]。
///
/// - 电影类（movie_hot_gaia / movie_showing / movie_soon）：`cover.url`、`info`、`description`
/// - 剧集/综艺类：`pic.large|pic.normal`、`card_subtitle`、`comment`
/// - TOP250：`cover_url`、`card_subtitle`、`description`
fn normalize_item(raw: &Value) -> Option<DoubanItem> {
    let id = string_of(raw.get("id"))?;
    let title = string_of(raw.get("title"))?;

    let cover = pick_cover(raw);
    let rating = raw
        .get("rating")
        .and_then(|rating| rating.get("value"))
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0);
    let year = string_of(raw.get("year")).or_else(|| year_from_card_subtitle(raw));
    let info = info_of(raw);
    let intro = string_of(raw.get("description")).or_else(|| string_of(raw.get("comment")));
    let subtype = string_of(raw.get("subtype")).or_else(|| string_of(raw.get("type")));

    Some(DoubanItem {
        id,
        title,
        cover,
        rating,
        year,
        info,
        intro,
        subtype,
    })
}

fn pick_cover(raw: &Value) -> Option<String> {
    let url = raw
        .pointer("/cover/url")
        .and_then(Value::as_str)
        .or_else(|| raw.pointer("/pic/large").and_then(Value::as_str))
        .or_else(|| raw.pointer("/pic/normal").and_then(Value::as_str))
        .or_else(|| raw.get("cover_url").and_then(Value::as_str))?;
    Some(upgrade_cover(url))
}

/// 小尺寸封面换中图，避免首页卡片糊成一团。
fn upgrade_cover(url: &str) -> String {
    url.replace("/s_ratio_poster/", "/m_ratio_poster/")
}

fn year_from_card_subtitle(raw: &Value) -> Option<String> {
    let card = string_of(raw.get("card_subtitle"))?;
    let first = card.split(" / ").next()?.trim();
    if first.len() == 4 && first.chars().all(|c| c.is_ascii_digit()) {
        Some(first.to_string())
    } else {
        None
    }
}

/// `info` 优先用接口自带的；没有就从 `card_subtitle` 里去掉年份那一段。
fn info_of(raw: &Value) -> Option<String> {
    if let Some(info) = string_of(raw.get("info")) {
        return Some(info);
    }
    let card = string_of(raw.get("card_subtitle"))?;
    let mut parts: Vec<&str> = card
        .split(" / ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts
        .first()
        .map(|part| part.len() == 4 && part.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
    {
        parts.remove(0);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

/// 补全轮播条目的详情；失败时保留集合里的数据。
fn enrich_item(client: &reqwest::blocking::Client, item: &mut DoubanItem, kind: &str) {
    let url = format!("{SUBJECT_API}/{kind}/{}", item.id);
    let detail = match get_json(client, &url) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!("豆瓣详情补全失败 {}: {err}", item.id);
            return;
        }
    };

    if let Some(title) = string_of(detail.get("title")) {
        item.title = title;
    }
    if let Some(intro) = string_of(detail.get("intro")) {
        item.intro = Some(intro);
    }
    if let Some(year) = string_of(detail.get("year")) {
        item.year = Some(year);
    }
    if let Some(cover) = detail.get("cover_url").and_then(Value::as_str) {
        item.cover = Some(upgrade_cover(cover));
    }
    if let Some(rating) = detail
        .get("rating")
        .and_then(|rating| rating.get("value"))
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0)
    {
        item.rating = Some(rating);
    }

    let countries = names_of(&detail, "countries");
    let genres = names_of(&detail, "genres");
    let directors = names_of(&detail, "directors");
    let actors = names_of(&detail, "actors");
    let info = build_info(&countries, &genres, &directors, &actors);
    if !info.is_empty() {
        item.info = Some(info);
    }
}

/// 国家 / 类型 / 导演 / 前 3 主演 拼成一句，空段自动省略。
fn build_info(
    countries: &[String],
    genres: &[String],
    directors: &[String],
    actors: &[String],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !countries.is_empty() {
        parts.push(countries.join(" "));
    }
    if !genres.is_empty() {
        parts.push(genres.join(" "));
    }
    if !directors.is_empty() {
        parts.push(directors.join(" "));
    }
    if !actors.is_empty() {
        parts.push(actors.iter().take(3).cloned().collect::<Vec<_>>().join(" "));
    }
    parts.join(" / ")
}

/// 取字符串数组；同时兼容 `["剧情"]` 和 `[{"name":"张三"}]` 两种形状。
fn names_of(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| item.get("name").and_then(Value::as_str).map(str::to_string))
                })
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn string_of(value: Option<&Value>) -> Option<String> {
    let text = match value? {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 电影类集合形状：cover.url / info / description / rating.value / year。
    #[test]
    fn normalize_movie_shape() {
        let raw = json!({
            "id": "37446297",
            "title": "逃出布迪秀",
            "type": "movie",
            "subtype": "",
            "year": "2026",
            "info": "美国 / 惊悚 恐怖 奇幻 / 加斯帕·凯利 / 德莱尼·奎因",
            "description": "一段剧情简介。",
            "cover": {
                "url": "https://img3.doubanio.com/view/photo/s_ratio_poster/public/p1.jpg"
            },
            "rating": { "value": 7.4, "count": 27860 }
        });

        let item = normalize_item(&raw).expect("movie 形状应能解析");
        assert_eq!(item.id, "37446297");
        assert_eq!(item.title, "逃出布迪秀");
        assert_eq!(item.year.as_deref(), Some("2026"));
        assert_eq!(item.rating, Some(7.4));
        assert_eq!(
            item.cover.as_deref(),
            Some("https://img3.doubanio.com/view/photo/m_ratio_poster/public/p1.jpg")
        );
        assert_eq!(
            item.info.as_deref(),
            Some("美国 / 惊悚 恐怖 奇幻 / 加斯帕·凯利 / 德莱尼·奎因")
        );
        assert_eq!(item.intro.as_deref(), Some("一段剧情简介。"));
        assert_eq!(item.subtype.as_deref(), Some("movie"));
    }

    /// 剧集类形状：pic.large / card_subtitle / comment / 无 subtype。
    #[test]
    fn normalize_tv_shape() {
        let raw = json!({
            "id": 36675942,
            "title": "小城良方",
            "type": "tv",
            "year": "2026",
            "card_subtitle": "2026 / 中国大陆 / 剧情 / 张晓波 / 肖战 倪大红",
            "comment": "该剧讲述了天才医生返乡的故事。",
            "pic": {
                "large": "https://img2.doubanio.com/view/photo/s_ratio_poster/public/p2.jpg",
                "normal": "https://img2.doubanio.com/view/photo/s_ratio_poster/public/p2n.jpg"
            },
            "rating": { "value": 8.0 }
        });

        let item = normalize_item(&raw).expect("tv 形状应能解析");
        assert_eq!(item.id, "36675942");
        assert_eq!(item.year.as_deref(), Some("2026"));
        assert_eq!(
            item.cover.as_deref(),
            Some("https://img2.doubanio.com/view/photo/m_ratio_poster/public/p2.jpg")
        );
        assert_eq!(
            item.info.as_deref(),
            Some("中国大陆 / 剧情 / 张晓波 / 肖战 倪大红")
        );
        assert_eq!(
            item.intro.as_deref(),
            Some("该剧讲述了天才医生返乡的故事。")
        );
        assert_eq!(item.subtype.as_deref(), Some("tv"));
    }

    /// TOP250 形状：cover_url / card_subtitle / description / rank。
    #[test]
    fn normalize_top250_shape() {
        let raw = json!({
            "id": "1292052",
            "title": "肖申克的救赎",
            "type": "movie",
            "subtype": "movie",
            "rank": 1,
            "card_subtitle": "1994 / 美国 / 剧情 犯罪 / 弗兰克·德拉邦特 / 蒂姆·罗宾斯",
            "description": "希望让人自由。",
            "cover_url": "https://img3.doubanio.com/view/photo/s_ratio_poster/public/p3.jpg",
            "rating": { "value": 9.7 }
        });

        let item = normalize_item(&raw).expect("TOP250 形状应能解析");
        assert_eq!(item.year.as_deref(), Some("1994"));
        assert_eq!(item.rating, Some(9.7));
        assert_eq!(item.intro.as_deref(), Some("希望让人自由。"));
        assert_eq!(
            item.cover.as_deref(),
            Some("https://img3.doubanio.com/view/photo/m_ratio_poster/public/p3.jpg")
        );
        assert_eq!(
            item.info.as_deref(),
            Some("美国 / 剧情 犯罪 / 弗兰克·德拉邦特 / 蒂姆·罗宾斯")
        );
    }

    /// 无评分 / 无封面的条目也要能出，不能因为字段缺失整条丢掉。
    #[test]
    fn normalize_tolerates_missing_optional_fields() {
        let raw = json!({ "id": "1", "title": "无评分条目" });
        let item = normalize_item(&raw).expect("最少字段也应能解析");
        assert_eq!(item.rating, None);
        assert_eq!(item.cover, None);
        assert_eq!(item.year, None);

        assert!(
            normalize_item(&json!({ "id": "2" })).is_none(),
            "缺 title 应丢弃"
        );
        assert!(
            normalize_item(&json!({ "title": "缺 id" })).is_none(),
            "缺 id 应丢弃"
        );
    }

    #[test]
    fn build_info_skips_empty_parts() {
        assert_eq!(
            build_info(&["中国大陆".into()], &["剧情".into()], &[], &[]),
            "中国大陆 / 剧情"
        );
        assert_eq!(build_info(&[], &[], &[], &[]), "");
    }

    /// 真实网络测试（默认不跑）：
    /// `cargo test -p vbox_core douban -- --ignored --nocapture`
    #[test]
    #[ignore = "真实网络测试：cargo test -p vbox_core douban -- --ignored --nocapture"]
    fn douban_live_home() {
        let first = home(true).expect("真实网络抓取豆瓣首页失败");

        println!("collections = {}", first.collections.len());
        for collection in &first.collections {
            println!(
                "  {} ({}) = {} 条，首条：{}",
                collection.key,
                collection.name,
                collection.items.len(),
                collection
                    .items
                    .first()
                    .map(|item| item.title.as_str())
                    .unwrap_or("")
            );
            assert!(!collection.items.is_empty(), "{} 不应为空", collection.key);
        }

        println!("carousel = {}", first.carousel.len());
        for item in &first.carousel {
            println!(
                "  {} | {} | cover={} | intro={} 字 | rating={:?} | info={}",
                item.id,
                item.title,
                item.cover.is_some(),
                item.intro
                    .as_ref()
                    .map(|intro| intro.chars().count())
                    .unwrap_or(0),
                item.rating,
                item.info.as_deref().unwrap_or("")
            );
        }

        assert_eq!(
            first.collections.len(),
            COLLECTION_KEYS.len(),
            "应拿到全部 8 个集合"
        );
        assert_eq!(first.carousel.len(), 10, "轮播应为 5 + 5 共 10 条");
        assert!(
            first.carousel.iter().all(|item| item.cover.is_some()),
            "轮播条目都应带封面"
        );
        assert!(
            first.carousel.iter().any(|item| item.intro.is_some()),
            "轮播条目应至少有一条详情简介"
        );
        assert!(!first.cache_hit);

        let cached = home(false).expect("读取缓存失败");
        assert!(cached.cache_hit, "10 分钟内应命中缓存");
        assert_eq!(cached.carousel.len(), first.carousel.len());
    }

    /* ---------------- 持久化缓存 / 增量刷新（不联网） ---------------- */

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 唯一临时目录（测试结束由调用方清理）。
    fn temp_dir(tag: &str) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join(format!(
                "vbox-douban-{}-{}-{seq}",
                std::process::id(),
                now_millis()
            ))
            .join(tag);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn item(id: &str, title: &str, intro: Option<&str>) -> DoubanItem {
        DoubanItem {
            id: id.to_string(),
            title: title.to_string(),
            intro: intro.map(str::to_string),
            ..DoubanItem::default()
        }
    }

    fn collection(
        key: &str,
        name: &str,
        fetched_at: u64,
        items: Vec<DoubanItem>,
    ) -> CachedCollection {
        CachedCollection {
            key: key.to_string(),
            name: name.to_string(),
            fetched_at,
            items,
        }
    }

    /// 磁盘缓存 save/load 往返；字段为 camelCase；cached_home 可直接渲染。
    #[test]
    fn disk_cache_roundtrip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("douban-home.json");

        let mut cache = DiskCache::default();
        cache.collections.push(collection(
            "movie_hot_gaia",
            "热门电影",
            1_000,
            vec![item("m1", "电影一", None)],
        ));
        cache.carousel.push(CachedCarouselItem {
            source: "movie".to_string(),
            enriched_at: 2_000,
            item: item("m1", "电影一", Some("简介")),
        });
        write_disk_cache(&path, &cache);

        let loaded = read_disk_cache(&path).expect("应能读回磁盘缓存");
        assert_eq!(loaded.version, CACHE_FILE_VERSION);
        assert_eq!(loaded.collections.len(), 1);
        assert_eq!(loaded.collections[0].fetched_at, 1_000);
        assert_eq!(loaded.carousel[0].enriched_at, 2_000);
        assert_eq!(loaded.carousel[0].item.intro.as_deref(), Some("简介"));

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"fetchedAt\""), "应为 camelCase 键");
        assert!(text.contains("\"enrichedAt\""));
        assert!(
            !path.with_extension("json.tmp").exists(),
            "临时文件不应残留"
        );

        let home = cached_home(&path).expect("cached_home 应能组装首页");
        assert!(home.cache_hit);
        assert_eq!(home.collections.len(), 1);
        assert_eq!(home.collections[0].key, "movie_hot_gaia");
        assert_eq!(home.carousel.len(), 1);
        assert!(home.image_proxy.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 版本不符 / 损坏 JSON / 没有集合 → cached_home 返回 None。
    #[test]
    fn disk_cache_rejects_bad_version_and_corrupt_json() {
        let dir = temp_dir("bad");
        let path = dir.join("douban-home.json");

        std::fs::write(
            &path,
            r#"{"version":1,"savedAt":0,"collections":[],"carousel":[]}"#,
        )
        .unwrap();
        assert!(read_disk_cache(&path).is_none(), "旧版本应视为无缓存");
        assert!(cached_home(&path).is_none());

        std::fs::write(&path, "{ not json").unwrap();
        assert!(read_disk_cache(&path).is_none(), "损坏 JSON 应视为无缓存");
        assert!(cached_home(&path).is_none());

        // 版本正确但没有集合数据：cached_home 不可用。
        write_disk_cache(&path, &DiskCache::default());
        assert!(read_disk_cache(&path).is_some());
        assert!(cached_home(&path).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 组装顺序按 COLLECTION_KEYS，fetchedAt 取各集合最大值，未知集合忽略。
    #[test]
    fn build_home_orders_collections_and_uses_latest_fetched_at() {
        let mut cache = DiskCache::default();
        cache.collections = vec![
            collection("tv_hot", "热门剧集", 300, vec![item("tv1", "剧一", None)]),
            collection(
                "movie_hot_gaia",
                "热门电影",
                900,
                vec![item("m1", "影一", None)],
            ),
            collection("unknown_key", "未知", 9_999, vec![item("x1", "未知", None)]),
        ];
        cache.carousel.push(CachedCarouselItem {
            source: "movie".to_string(),
            enriched_at: 1,
            item: item("m1", "影一", Some("简介")),
        });

        let home = build_home(&cache, true);
        assert!(home.cache_hit);
        assert_eq!(home.collections.len(), 2, "未知集合不应出现在首页");
        assert_eq!(home.collections[0].key, "movie_hot_gaia");
        assert_eq!(home.collections[1].key, "tv_hot");
        assert_eq!(home.fetched_at, 900, "fetchedAt 应取最大值");
        assert_eq!(home.carousel.len(), 1);
        assert!(home.image_proxy.is_none());
    }

    /// 过期判断：缺失 / 正好 TTL / 超过 TTL 视为过期，未到 TTL 不算。
    #[test]
    fn stale_detection_uses_fetched_at() {
        let now = 1_000_000u64;
        let mut cache = DiskCache::default();
        assert!(
            is_stale(&cache, "movie_hot_gaia", now),
            "缓存缺失应视为过期"
        );

        cache
            .collections
            .push(collection("movie_hot_gaia", "热门电影", now, vec![]));
        assert!(!is_stale(&cache, "movie_hot_gaia", now), "刚抓过不应过期");

        cache.collections[0].fetched_at = now - CACHE_TTL_MS;
        assert!(
            is_stale(&cache, "movie_hot_gaia", now),
            "正好 10 分钟应过期"
        );

        cache.collections[0].fetched_at = now - CACHE_TTL_MS + 1;
        assert!(
            !is_stale(&cache, "movie_hot_gaia", now),
            "不到 10 分钟不应过期"
        );
    }

    /// 轮播种子：热门电影前 5 + 热门剧集前 5，其他集合不参与。
    #[test]
    fn seed_items_takes_top_five_from_two_collections() {
        let movies: Vec<DoubanItem> = (0..8)
            .map(|i| item(&format!("m{i}"), "电影", None))
            .collect();
        let tvs: Vec<DoubanItem> = (0..8)
            .map(|i| item(&format!("t{i}"), "剧集", None))
            .collect();
        let collections = vec![
            collection("movie_hot_gaia", "热门电影", 1, movies),
            collection("tv_hot", "热门剧集", 1, tvs),
            collection("show_hot", "热门综艺", 1, vec![item("s1", "综艺", None)]),
        ];

        let seeds = seed_items(&collections);
        assert_eq!(seeds.len(), 10, "应为 5 + 5");
        assert!(seeds[..5].iter().all(|(_, kind)| *kind == "movie"));
        assert!(seeds[5..].iter().all(|(_, kind)| *kind == "tv"));
        assert_eq!(seeds[4].0.id, "m4");
        assert_eq!(seeds[9].0.id, "t4");
    }

    /// 轮播复用：有 intro 且非 force 才复用；force 或无 intro 一律补全。
    #[test]
    fn carousel_reuse_logic() {
        let cached = vec![CachedCarouselItem {
            source: "movie".to_string(),
            enriched_at: 42,
            item: item("m1", "旧标题", Some("已补全简介")),
        }];
        let enriched_seed = item("m1", "新标题", None);
        let plain_seed = item("m2", "新条目", None);

        let reused = carousel_reuse(&cached, &enriched_seed, "movie", false).expect("应复用");
        assert_eq!(reused.enriched_at, 42, "复用应保留原 enrichedAt");
        assert_eq!(reused.item.title, "旧标题");

        assert!(
            carousel_reuse(&cached, &plain_seed, "movie", false).is_none(),
            "没有 intro 的条目不应复用"
        );
        assert!(
            carousel_reuse(&cached, &enriched_seed, "movie", true).is_none(),
            "force 时不应复用"
        );
    }

    /// 全部集合都新鲜时完全走磁盘缓存（不联网），并标记 cacheHit。
    #[test]
    fn home_with_cache_uses_fresh_disk_cache_without_network() {
        let dir = temp_dir("fresh");
        let path = dir.join("douban-home.json");
        let now = now_millis();

        let mut cache = DiskCache::default();
        for (key, name) in COLLECTION_KEYS {
            cache
                .collections
                .push(collection(key, name, now, vec![item(key, name, None)]));
        }
        cache.carousel.push(CachedCarouselItem {
            source: "movie".to_string(),
            enriched_at: now,
            item: item("movie_hot_gaia", "热门电影", Some("简介")),
        });
        write_disk_cache(&path, &cache);

        let home = home_with_cache(Some(path.as_path()), false)
            .expect("新鲜的磁盘缓存应直接命中（不联网）");
        assert!(home.cache_hit);
        assert_eq!(home.collections.len(), COLLECTION_KEYS.len());
        assert_eq!(home.collections[0].key, "movie_hot_gaia");
        assert_eq!(home.carousel.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
