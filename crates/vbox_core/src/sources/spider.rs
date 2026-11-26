//! type=3 源适配器：Spider（jar / js / py）
//!
//! 桌面端现实边界：
//! - `.js`：交给 `spider::js`（rquickjs 子集）
//! - `.py`：交给 `python`（系统 CPython sidecar，见 `python/mod.rs`）
//! - `csp_XYQHiker*` / `csp_XBPQ`：原生规则引擎（xyq / xbpq）
//! - 其余 `csp_*.jar`：交给 `catvod::jvm`（真 JRE + 兼容层执行原始字节码，主力车道），
//!   失败后依次回退 `catvod::descriptor` 描述层、规则引擎、脚本运行时；
//!   全部落空时返回带稳定错误码的分类错误（见 `catvod::site_error`）。
//!   注意 JVM 分支必须排在 js/py 启发式**之前**——jar spider 的 `ext` 常常就是一个
//!   站点 URL，而 `python::is_python_site` 会把「ext 是 http 地址」误判成 py 脚本源。
//! - 站点级 `jar` 同理
use crate::error::Result;
use crate::model::{Category, Site, TvConfig, VideoDetail, VideoItem};
use super::map_source_error;

/// jar spider（`csp_*`）的 `ext` 经常就是站点 URL/配置文件。
/// python 的 `is_python_site` 把「ext 是 http(s) 地址」当成 py 脚本源，
/// 若不在车道判定里排除 `csp_*`，一些没有可用 JVM 入口的 jar 站点
/// 会被误送进 Python 运行时，返回 Python 侧错误而不是分类错误码。
fn script_lane_allowed(site: &Site) -> bool {
    !site.api.trim().starts_with("csp_")
}

fn api_is_js(site: &Site) -> bool {
    if !script_lane_allowed(site) {
        return false;
    }
    let api = site.api.trim();
    api.ends_with(".js") || api.contains(".js?")
}

fn api_is_py(site: &Site) -> bool {
    if !script_lane_allowed(site) {
        return false;
    }
    crate::python::is_python_site(site)
}

fn api_is_xyq(site: &Site) -> bool {
    matches!(
        site.api.trim(),
        "csp_XYQHiker" | "csp_XYQHikerAL" | "csp_XYQHikerV2"
    )
}

fn api_is_xbpq(site: &Site) -> bool {
    matches!(site.api.trim(), "csp_XBPQ" | "csp_XBPQ_upgraded")
}

fn api_is_drpy(site: &Site) -> bool {
    if !script_lane_allowed(site) {
        return false;
    }
    crate::node::is_drpy_site(&site.api, &site.ext_string())
}


fn list_categories_dispatch(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    // JVM 主力车道：站点带 jar 时先跑真 JRE 的原始字节码；失败后回退描述/规则/脚本车道。
    let mut jvm_lead_err: Option<crate::error::CoreError> = None;
    // JVM 跑通但分类为空：不是硬失败，但通常意味着蜘蛛要 ext/配置才吐分类（如 Auete）。
    // 此时继续回退描述/规则/脚本车道；都没有结果再返回这个空表——比直接给用户空目录好。
    let mut jvm_lead_empty: Option<Vec<Category>> = None;
    if crate::spider::catvod::jvm::should_lead(site) {
        match crate::spider::catvod::jvm::list_categories(cfg, site) {
            Ok(v) if !v.is_empty() => {
                tracing::debug!(site = site.key.as_str(), method = "list_categories", "JVM 主力车道命中");
                return Ok(v);
            }
            Ok(v) => {
                tracing::debug!(site = site.key.as_str(), "JVM 主力返回空分类，回退后续车道再看");
                jvm_lead_empty = Some(v);
            }
            Err(e) => {
                tracing::debug!(site = site.key.as_str(), error = ?e, "JVM 主力车道失败，回退");
                jvm_lead_err = Some(e.into_core());
            }
        }
    }
    if let Some(result) = crate::spider::catvod::descriptor::try_categories(cfg, site) {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::list_categories(cfg, site)
    } else if api_is_xyq(site) {
        crate::sources::xyq::list_categories(cfg, site)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::list_categories(cfg, site)
    } else if api_is_js(site) {
        crate::spider::js::categories(site, cfg)
    } else if api_is_py(site) {
        crate::python::categories(cfg, site)
    } else {
        if let Some(err) = jvm_lead_err {
            return Err(err);
        }
        if let Some(v) = jvm_lead_empty {
            return Ok(v);
        }
        if crate::spider::catvod::jvm::can_handle(site) {
            return crate::spider::catvod::jvm::list_categories(cfg, site).map_err(|e| e.into_core());
        }
        Err(crate::spider::catvod::site_error(site))
    }
}

fn list_videos_dispatch(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    // JVM 主力车道：站点带 jar 时先跑真 JRE 的原始字节码；失败后回退描述/规则/脚本车道。
    let mut jvm_lead_err: Option<crate::error::CoreError> = None;
    if crate::spider::catvod::jvm::should_lead(site) {
        match crate::spider::catvod::jvm::list_videos(cfg, site, cat, page) {
            Ok(v) => {
                tracing::debug!(site = site.key.as_str(), method = "list_videos", "JVM 主力车道命中");
                return Ok(v);
            }
            Err(e) => {
                tracing::debug!(site = site.key.as_str(), error = ?e, "JVM 主力车道失败，回退");
                jvm_lead_err = Some(e.into_core());
            }
        }
    }
    if let Some(result) = crate::spider::catvod::descriptor::try_videos(cfg, site, cat, page) {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::list_videos(cfg, site, cat, page)
    } else if api_is_xyq(site) {
        crate::sources::xyq::list_videos(cfg, site, cat, page)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::list_videos(cfg, site, cat, page)
    } else if api_is_js(site) {
        crate::spider::js::category(site, cfg, cat, page)
    } else if api_is_py(site) {
        crate::python::list_videos(cfg, site, cat, page)
    } else {
        if let Some(err) = jvm_lead_err {
            return Err(err);
        }
        if crate::spider::catvod::jvm::can_handle(site) {
            return crate::spider::catvod::jvm::list_videos(cfg, site, cat, page).map_err(|e| e.into_core());
        }
        Err(crate::spider::catvod::site_error(site))
    }
}

fn get_detail_dispatch(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    // JVM 主力车道：站点带 jar 时先跑真 JRE 的原始字节码；失败后回退描述/规则/脚本车道。
    let mut jvm_lead_err: Option<crate::error::CoreError> = None;
    if crate::spider::catvod::jvm::should_lead(site) {
        match crate::spider::catvod::jvm::get_detail(cfg, site, id) {
            Ok(v) => {
                tracing::debug!(site = site.key.as_str(), method = "get_detail", "JVM 主力车道命中");
                return Ok(v);
            }
            Err(e) => {
                tracing::debug!(site = site.key.as_str(), error = ?e, "JVM 主力车道失败，回退");
                jvm_lead_err = Some(e.into_core());
            }
        }
    }
    if let Some(result) = crate::spider::catvod::descriptor::try_detail(cfg, site, id) {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::get_detail(cfg, site, id)
    } else if api_is_xyq(site) {
        crate::sources::xyq::get_detail(cfg, site, id)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::get_detail(cfg, site, id)
    } else if api_is_js(site) {
        crate::spider::js::detail(site, cfg, id)
    } else if api_is_py(site) {
        crate::python::get_detail(cfg, site, id)
    } else {
        if let Some(err) = jvm_lead_err {
            return Err(err);
        }
        if crate::spider::catvod::jvm::can_handle(site) {
            return crate::spider::catvod::jvm::get_detail(cfg, site, id).map_err(|e| e.into_core());
        }
        Err(crate::spider::catvod::site_error(site))
    }
}

fn search_dispatch(cfg: &TvConfig, site: &Site, keyword: &str, quick: bool) -> Result<Vec<VideoItem>> {
    // JVM 主力车道：站点带 jar 时先跑真 JRE 的原始字节码；失败后回退描述/规则/脚本车道。
    let mut jvm_lead_err: Option<crate::error::CoreError> = None;
    if crate::spider::catvod::jvm::should_lead(site) {
        match crate::spider::catvod::jvm::search(cfg, site, keyword, quick) {
            Ok(v) => {
                tracing::debug!(site = site.key.as_str(), method = "search", "JVM 主力车道命中");
                return Ok(v);
            }
            Err(e) => {
                tracing::debug!(site = site.key.as_str(), error = ?e, "JVM 主力车道失败，回退");
                jvm_lead_err = Some(e.into_core());
            }
        }
    }
    if let Some(result) = crate::spider::catvod::descriptor::try_search(cfg, site, keyword) {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::search(cfg, site, keyword, quick)
    } else if api_is_xyq(site) {
        crate::sources::xyq::search(cfg, site, keyword, quick)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::search(cfg, site, keyword, quick)
    } else if api_is_js(site) {
        crate::spider::js::search(site, cfg, keyword, quick)
    } else if api_is_py(site) {
        crate::python::search(cfg, site, keyword, quick)
    } else {
        if let Some(err) = jvm_lead_err {
            return Err(err);
        }
        if crate::spider::catvod::jvm::can_handle(site) {
            return crate::spider::catvod::jvm::search(cfg, site, keyword, quick).map_err(|e| e.into_core());
        }
        Err(crate::spider::catvod::site_error(site))
    }
}

/// 播放信息：type3 需要 spider.playerContent；桌面端 js / XYQHiker / 描述层走原生实现
fn player_content_dispatch(
    cfg: &TvConfig,
    site: &Site,
    flag: &str,
    id: &str,
    vip_flags: &[String],
) -> Result<serde_json::Value> {
    // JVM 主力车道：站点带 jar 时先跑真 JRE 的原始字节码；失败后回退描述/规则/脚本车道。
    let mut jvm_lead_err: Option<crate::error::CoreError> = None;
    if crate::spider::catvod::jvm::should_lead(site) {
        match crate::spider::catvod::jvm::player_content(cfg, site, flag, id, vip_flags) {
            Ok(v) => {
                tracing::debug!(site = site.key.as_str(), method = "player_content", "JVM 主力车道命中");
                return Ok(v);
            }
            Err(e) => {
                tracing::debug!(site = site.key.as_str(), error = ?e, "JVM 主力车道失败，回退");
                jvm_lead_err = Some(e.into_core());
            }
        }
    }
    if let Some(result) =
        crate::spider::catvod::descriptor::try_player_content(cfg, site, flag, id)
    {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::player_content(cfg, site, flag, id, vip_flags)
    } else if api_is_xyq(site) {
        crate::sources::xyq::player_content(cfg, site, flag, id, vip_flags)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::player_content(cfg, site, flag, id, vip_flags)
    } else if api_is_js(site) {
        crate::spider::js::player_content(site, cfg, flag, id, vip_flags)
    } else if api_is_py(site) {
        crate::python::player_content(cfg, site, flag, id, vip_flags)
    } else {
        if let Some(err) = jvm_lead_err {
            return Err(err);
        }
        if crate::spider::catvod::jvm::can_handle(site) {
            return crate::spider::catvod::jvm::player_content(cfg, site, flag, id, vip_flags).map_err(|e| e.into_core());
        }
        Err(crate::spider::catvod::site_error(site))
    }
}

pub fn list_categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    list_categories_dispatch(cfg, site).map_err(|e| map_source_error(site, e))
}

pub fn list_videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    list_videos_dispatch(cfg, site, cat, page).map_err(|e| map_source_error(site, e))
}

pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    get_detail_dispatch(cfg, site, id).map_err(|e| map_source_error(site, e))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, quick: bool) -> Result<Vec<VideoItem>> {
    search_dispatch(cfg, site, keyword, quick).map_err(|e| map_source_error(site, e))
}

pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    flag: &str,
    id: &str,
    vip_flags: &[String],
) -> Result<serde_json::Value> {
    player_content_dispatch(cfg, site, flag, id, vip_flags).map_err(|e| map_source_error(site, e))
}
