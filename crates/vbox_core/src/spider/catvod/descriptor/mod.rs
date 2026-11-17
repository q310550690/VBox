//! 描述驱动的家族实现（T1：翻译层与引擎）。
//!
//! 执行顺序：分发入口先问本模块要结果，拿到 `Some` 即返回；拿到 `None` 表示
//! 该 api 没有描述（或描述执行失败且未开严格模式），由既有手写家族兜底。
//!
//! 开关：
//! - `VBOX_DESCRIPTOR=0|off|false|no`：整体关闭描述层，全部走手写。
//! - `VBOX_DESCRIPTOR_STRICT=1`：描述执行失败时直接返回错误（测试用，防止
//!   手写兜底掩盖描述缺陷）。

pub mod engine;
pub mod path;
pub mod registry;
pub mod schema;
pub mod value;

use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::model::{Category, Site, TvConfig, VideoDetail, VideoItem};

/// 描述层是否启用。
pub fn enabled() -> bool {
    match std::env::var("VBOX_DESCRIPTOR") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no"
        ),
        Err(_) => true,
    }
}

/// 失败时是否透传错误。
fn strict() -> bool {
    matches!(
        std::env::var("VBOX_DESCRIPTOR_STRICT").as_deref(),
        Ok("1" | "true" | "yes")
    )
}

fn resolve(site: &Site) -> Option<&'static schema::Descriptor> {
    if !enabled() {
        return None;
    }
    registry::find(&site.api)
}

/// 描述失败后的统一处置：严格模式透传错误；否则告警并返回 `None` 让手写兜底。
fn on_error<T>(desc: &schema::Descriptor, site: &Site, entry: &str, error: CoreError) -> Option<Result<T>> {
    if strict() {
        return Some(Err(error));
    }
    tracing::warn!(
        api = desc.api.as_str(),
        entry,
        error = %error,
        "描述执行失败，回退手写家族（api={}）",
        site.api.trim()
    );
    None
}

/// 分类入口。
pub fn try_categories(cfg: &TvConfig, site: &Site) -> Option<Result<Vec<Category>>> {
    let desc = resolve(site)?;
    if engine::delegates(desc, engine::Entry::Categories) {
        return None;
    }
    match engine::categories(desc, cfg, site) {
        Ok(value) => Some(Ok(value)),
        Err(error) => on_error(desc, site, "categories", error),
    }
}

/// 列表入口。
pub fn try_videos(
    cfg: &TvConfig,
    site: &Site,
    cat: &str,
    page: u32,
) -> Option<Result<Vec<VideoItem>>> {
    let desc = resolve(site)?;
    if engine::delegates(desc, engine::Entry::List) {
        return None;
    }
    match engine::videos(desc, cfg, site, cat, page) {
        Ok(value) => Some(Ok(value)),
        Err(error) => on_error(desc, site, "videos", error),
    }
}

/// 详情入口。
pub fn try_detail(cfg: &TvConfig, site: &Site, id: &str) -> Option<Result<VideoDetail>> {
    let desc = resolve(site)?;
    if engine::delegates(desc, engine::Entry::Detail) {
        return None;
    }
    match engine::detail(desc, cfg, site, id) {
        Ok(value) => Some(Ok(value)),
        Err(error) => on_error(desc, site, "detail", error),
    }
}

/// 搜索入口。
pub fn try_search(cfg: &TvConfig, site: &Site, keyword: &str) -> Option<Result<Vec<VideoItem>>> {
    let desc = resolve(site)?;
    if engine::delegates(desc, engine::Entry::Search) {
        return None;
    }
    match engine::search(desc, cfg, site, keyword) {
        Ok(value) => Some(Ok(value)),
        Err(error) => on_error(desc, site, "search", error),
    }
}

/// 播放入口。
pub fn try_player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Option<Result<Value>> {
    let desc = resolve(site)?;
    if engine::delegates(desc, engine::Entry::Play) {
        return None;
    }
    match engine::player_content(desc, cfg, site, id) {
        Ok(value) => Some(Ok(value)),
        Err(error) => on_error(desc, site, "player", error),
    }
}
