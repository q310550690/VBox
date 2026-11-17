//! 各源适配器共用的小工具：把站点/搜索请求路由到具体实现

use std::collections::HashMap;

use crate::error::{CoreError, Result};
use crate::model::{Category, Site, TvConfig, VideoItem};

pub mod cms; // type=1 苹果CMS
pub mod drpy; // drpy/drpy2（Node sidecar）
pub mod remote; // type=4 HTTP 远程接口
pub mod spider; // type=3 Spider（jar/js/py）
pub mod xbpq; // XBPQ 原生规则引擎（实验性）
pub mod xml; // type=0 XML
pub mod xyq; // XYQHiker 原生规则引擎（不执行 jar）

// ---------------------------------------------------------------------------
// 错误边界：各源适配器内部可能直接 `?` 出裸 `CoreError`（如 `HTTP 请求失败: …`、
// `源站返回错误: code=41006`），宿主侧无法归类。所有源入口统一经这里收口成
// catvod 稳定码：网络 → `catvod.net.*`；网盘语义（分享失效/Cookie）→
// `catvod.pan.*`；其余 → `catvod.result.parse`。已是 `catvod.*` 的原样透传。
// ---------------------------------------------------------------------------

/// 解析链（parses）错误的稳定码：网络层 → `catvod.net.*`，其余 → `catvod.play.parse`。
pub(crate) fn map_play_error(err: CoreError) -> CoreError {
    use crate::spider::catvod::contract::{CatvodError, ErrorClass};

    let msg = err.to_string();
    if msg.contains("catvod.") {
        return err;
    }
    let class = match &err {
        CoreError::Http(e) => ErrorClass::classify_reqwest_error(e),
        _ => ErrorClass::PlayParse,
    };
    let site = Site {
        key: "parse".into(),
        name: "解析链".into(),
        ..Default::default()
    };
    CatvodError::new(class, &site, msg).into_core()
}

/// 辅助资源（EPG/字幕等）错误的稳定码：网络层 → `catvod.net.*`，其余 → `catvod.aux.fetch`。
pub(crate) fn map_aux_error(resource: &str, err: CoreError) -> CoreError {
    use crate::spider::catvod::contract::{CatvodError, ErrorClass};

    let msg = err.to_string();
    if msg.contains("catvod.") {
        return err;
    }
    let class = match &err {
        CoreError::Http(e) => ErrorClass::classify_reqwest_error(e),
        _ => ErrorClass::AuxFetch,
    };
    let site = Site {
        key: resource.into(),
        name: resource.into(),
        ..Default::default()
    };
    CatvodError::new(class, &site, msg).into_core()
}

/// 把源适配器（spider 家族、脚本分支等）的裸错误映射为 catvod 稳定码。
pub(crate) fn map_source_error(site: &Site, err: CoreError) -> CoreError {
    use crate::spider::catvod::contract::{CatvodError, ErrorClass};

    let msg = err.to_string();
    if msg.contains("catvod.") {
        return err;
    }
    let class = match &err {
        CoreError::Http(e) => ErrorClass::classify_reqwest_error(e),
        // http::get_text_once 对非 2xx 返回带状态的 Config；不映射会被归成 Parse。
        CoreError::Config(msg) if msg.starts_with("HTTP ") && msg.contains("拉取失败") => {
            ErrorClass::Network
        }
        other => ErrorClass::classify_pan_error(other).unwrap_or(ErrorClass::Parse),
    };
    CatvodError::new(class, site, msg).into_core()
}

/// 把订阅里的 `categories` 映射到接口返回的真实分类
///
/// TVBox 的语义（`DefaultConfig.adjustSort`）：type0/1 的分类 id 必须来自接口
/// `class`，配置里的 `categories` 只是首页分类的白名单与顺序。字符串写法
/// （`["内地剧", ...]`）没有 id，直接用名称发 `t=内地剧` 会拿到空列表——
/// 这正是「分类能显示、内容为空」的根因。
///
/// 匹配顺序：名称 → id；都匹配不到时保留原条目（接口临时不可用时仍能显示分类），
/// 但不会凭空造 id，避免把错误隐藏成「空分类」。
pub(crate) fn merge_preset_categories(preset: &[Category], api: Vec<Category>) -> Vec<Category> {
    if preset.is_empty() {
        return api;
    }
    let by_name: HashMap<&str, &Category> = api
        .iter()
        .filter(|c| !c.type_name.trim().is_empty())
        .map(|c| (c.type_name.trim(), c))
        .collect();
    let by_id: HashMap<&str, &Category> = api
        .iter()
        .filter(|c| !c.type_id.trim().is_empty())
        .map(|c| (c.type_id.trim(), c))
        .collect();

    preset
        .iter()
        .map(|p| {
            let name = p.type_name.trim();
            if let Some(found) = by_name.get(name) {
                return (*found).clone();
            }
            let id = p.type_id.trim();
            if let Some(found) = by_id.get(id) {
                return (*found).clone();
            }
            p.clone()
        })
        .collect()
}

/// 单源搜索分发（type0/1/4 走 HTTP，type3 走 spider.search）
pub fn search_site(
    cfg: &TvConfig,
    site: &Site,
    keyword: &str,
    quick: bool,
) -> Result<Vec<VideoItem>> {
    match site.kind {
        0 => xml::search(cfg, site, keyword, quick).map_err(|e| map_source_error(site, e)),
        1 => cms::search(cfg, site, keyword, quick).map_err(|e| map_source_error(site, e)),
        4 => remote::search(cfg, site, keyword, quick).map_err(|e| map_source_error(site, e)),
        3 => spider::search(cfg, site, keyword, quick).map_err(|e| map_source_error(site, e)),
        k => Err(CoreError::Config(format!(
            "站点 {} 的类型 type={} 暂不支持搜索",
            site.name, k
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Site;

    fn site() -> Site {
        let mut s = Site::default();
        s.key = "t".into();
        s.name = "t".into();
        s.api = "csp_QuarkShare".into();
        s
    }

    #[test]
    fn api_41006_maps_to_pan_share_unavailable() {
        let err = CoreError::Api { code: 41006, msg: "分享不存在".into() };
        let mapped = map_source_error(&site(), err);
        assert!(mapped.to_string().contains("catvod.pan.share_unavailable"));
    }

    #[test]
    fn cookie_config_maps_to_pan_cookie_required() {
        let err = CoreError::Config("网盘需要 Cookie：请配置".into());
        let mapped = map_source_error(&site(), err);
        assert!(mapped.to_string().contains("catvod.pan.cookie_required"));
    }

    #[test]
    fn catvod_error_passes_through() {
        let err = CoreError::Config("catvod.result.parse：已经是稳定码".into());
        let mapped = map_source_error(&site(), err);
        assert!(mapped.to_string().contains("catvod.result.parse"));
    }
}
