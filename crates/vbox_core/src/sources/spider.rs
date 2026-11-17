//! type=3 源适配器：Spider（jar / js / py）
//!
//! 桌面端现实边界：
//! - `.js`：交给 `spider::js`（rquickjs 子集）
//! - `.py`：交给 `python`（系统 CPython sidecar，见 `python/mod.rs`）
//! - `csp_XYQHiker*` / `csp_XBPQ`：原生规则引擎（xyq / xbpq）
//! - 已移植的原生家族（`csp_PanSearch`、`csp_SixV`…，见 `spider::catvod::families`）：
//!   必须排在 js/py 启发式**之前**——jar spider 的 `ext` 常常就是一个站点 URL，
//!   而 `python::is_python_site` 会把「ext 是 http 地址」误判成 py 脚本源。
//! - 其余 `csp_*.jar`：桌面端不执行 dex；交给 `catvod` 契约层按画像分类（规则引擎/已移植家族/
//!   未移植/受保护/脚本/未知），返回带稳定错误码的分类错误（不再笼统提示「需 Android Host」）
//! - 站点级 `jar` 同理

use crate::error::Result;
use crate::model::{Category, Site, TvConfig, VideoDetail, VideoItem};
use super::map_source_error;
use crate::spider::catvod::families::pan as catvod_pan;
use crate::spider::catvod::families::pansearch as catvod_pansearch;
use crate::spider::catvod::families::sixv as catvod_sixv;
use crate::spider::catvod::families::anime1 as catvod_anime1;
use crate::spider::catvod::families::dm84 as catvod_dm84;
use crate::spider::catvod::families::bili as catvod_bili;
use crate::spider::catvod::families::firstaid as catvod_firstaid;
use crate::spider::catvod::families::ysj as catvod_ysj;
use crate::spider::catvod::families::jianpian as catvod_jianpian;
use crate::spider::catvod::families::jpys as catvod_jpys;
use crate::spider::catvod::families::yqkan as catvod_yqkan;
use crate::spider::catvod::families::libvio as catvod_libvio;
use crate::spider::catvod::families::nangua as catvod_nangua;
use crate::spider::catvod::families::apptt as catvod_apptt;
use crate::spider::catvod::families::appysv2 as catvod_appysv2;
use crate::spider::catvod::families::ying as catvod_ying;
use crate::spider::catvod::families::kanqiu as catvod_kanqiu;
use crate::spider::catvod::families::kkys as catvod_kkys;
use crate::spider::catvod::families::mp4mov as catvod_mp4mov;
use crate::spider::catvod::families::zxzj as catvod_zxzj;
use crate::spider::catvod::families::kugou as catvod_kugou;
use crate::spider::catvod::families::kunyu77 as catvod_kunyu77;
use crate::spider::catvod::families::douban as catvod_douban;
use crate::spider::catvod::families::sp360 as catvod_sp360;
use crate::spider::catvod::families::appget as catvod_appget;
use crate::spider::catvod::families::duanju as catvod_duanju;
use crate::spider::catvod::families::xb6v as catvod_xb6v;
use crate::spider::catvod::families::meijumi as catvod_meijumi;
use crate::spider::catvod::families::ddrk as catvod_ddrk;
use crate::spider::catvod::families::auete as catvod_auete;
use crate::spider::catvod::families::iktv as catvod_iktv;
use crate::spider::catvod::families::yinyuetai as catvod_yinyuetai;
use crate::spider::catvod::families::alist as catvod_alist;
use crate::spider::catvod::families::tangrenjie as catvod_tangrenjie;
use crate::spider::catvod::families::quark as catvod_quark;
use crate::spider::catvod::families::share as catvod_share;
use crate::spider::catvod::families::moli as catvod_moli;
use crate::spider::catvod::families::xiaozhang as catvod_xiaozhang;
use crate::spider::catvod::families::ttian as catvod_ttian;
use crate::spider::catvod::families::wogg as catvod_wogg;
use crate::spider::catvod::families::xpanpan as catvod_xpanpan;
use crate::spider::catvod::families::yingso as catvod_yingso;
use crate::spider::catvod::families::appmao as catvod_appmao;
use crate::spider::catvod::families::appld as catvod_appld;
use crate::spider::catvod::families::appsk as catvod_appsk;
use crate::spider::catvod::families::bttwoo as catvod_bttwoo;
use crate::spider::catvod::families::yunpan as catvod_yunpan;
use crate::spider::catvod::families::fourkhdr as catvod_fourkhdr;
use crate::spider::catvod::families::push as catvod_push;
use crate::spider::catvod::families::xinsj as catvod_xinsj;
use crate::spider::catvod::families::seedhub as catvod_seedhub;
use crate::spider::catvod::families::yj1211 as catvod_yj1211;
use crate::spider::catvod::families::liteapple as catvod_liteapple;
use crate::spider::catvod::families::xpath as catvod_xpath;
use crate::spider::catvod::families::nmys as catvod_nmys;
use crate::spider::catvod::families::nini as catvod_nini;
use crate::spider::catvod::families::bdys01 as catvod_bdys01;
use crate::spider::catvod::families::czsapp as catvod_czsapp;
use crate::spider::catvod::families::gitcafe as catvod_gitcafe;
use crate::spider::catvod::families::alishare as catvod_alishare;
use crate::spider::catvod::families::pushshare as catvod_pushshare;
use crate::spider::catvod::families::panyq as catvod_panyq;
use crate::spider::catvod::families::p115share as catvod_p115share;
use crate::spider::catvod::families::pikpakshare as catvod_pikpakshare;
use crate::spider::catvod::families::sambashare as catvod_sambashare;
use crate::spider::catvod::families::youtube as catvod_youtube;
use crate::spider::catvod::families::thundershare as catvod_thundershare;
use crate::spider::catvod::families::tgyunpan as catvod_tgyunpan;
use crate::spider::catvod::families::bt0 as catvod_bt0;
use crate::spider::catvod::families::gaoqing as catvod_gaoqing;
use crate::spider::catvod::families::zhaozy as catvod_zhaozy;
use crate::spider::catvod::families::hdhive as catvod_hdhive;
use crate::spider::catvod::families::xbiu as catvod_xbiu;
use crate::spider::catvod::families::kuaikan as catvod_kuaikan;
use crate::spider::catvod::families::xiaozhitiao as catvod_xiaozhitiao;
use crate::spider::catvod::families::huban as catvod_huban;
use crate::spider::catvod::families::tzfile as catvod_tzfile;

/// jar spider（`csp_*`）的 `ext` 经常就是站点 URL/配置文件。
/// python 的 `is_python_site` 把「ext 是 http(s) 地址」当成 py 脚本源，
/// 若不在车道判定里排除 `csp_*`，一些【已画像未移植/受保护】的 jar 站点
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


/// 站点最终会被送到的运行时车道。
///
/// 这是对 `list_categories_dispatch` / `list_videos_dispatch` / `search_dispatch`
/// 分派分支顺序的静态投影：只做画像与启发式判定，不发起任何网络请求，
/// 供审计报告与判据复用（见判据 42/43）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchLane {
    Native,
    Classified,
    ScriptDrpy,
    ScriptJs,
    ScriptPy,
    Source,
    Unknown,
}

/// 判定一个站点会被送到哪条车道。
///
/// 顺序与运行时保持一致：
/// 1. `kind != 3` 不是 Spider 站点 → `Source`（cms/xml 适配器处理）；
/// 2. 画像分类为 rulepack → `Native`；unported/unsupported → `Classified`
///    （返回 `catvod.*` 分类错误码）；
/// 3. script → 依次按 drpy → js → py 匹配（`csp_*` 先被
///    `script_lane_allowed` 拦住，防止 ext 是 URL 的 jar 站点误入脚本车道）。
pub fn dispatch_lane(site: &Site) -> DispatchLane {
    use crate::spider::catvod::{classify, Strategy};
    if site.kind != 3 {
        return DispatchLane::Source;
    }
    match classify(site).strategy() {
        Strategy::Rulepack => DispatchLane::Native,
        Strategy::Unported | Strategy::Unsupported => DispatchLane::Classified,
        Strategy::Script => {
            if api_is_drpy(site) {
                DispatchLane::ScriptDrpy
            } else if api_is_js(site) {
                DispatchLane::ScriptJs
            } else if api_is_py(site) {
                DispatchLane::ScriptPy
            } else {
                DispatchLane::Unknown
            }
        }
    }
}
fn list_categories_dispatch(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    if let Some(result) = crate::spider::catvod::descriptor::try_categories(cfg, site) {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::list_categories(cfg, site)
    } else if api_is_xyq(site) {
        crate::sources::xyq::list_categories(cfg, site)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::list_categories(cfg, site)
    } else if catvod_sixv::handles(site.api.trim()) {
        Ok(catvod_sixv::categories())
    } else if catvod_ysj::handles(site.api.trim()) {
        catvod_ysj::categories(cfg, site)
    } else if catvod_anime1::handles(site.api.trim()) {
        Ok(catvod_anime1::categories())
    } else if catvod_dm84::handles(site.api.trim()) {
        catvod_dm84::categories(cfg, site)
    } else if catvod_bili::handles(site.api.trim()) {
        catvod_bili::categories(cfg, site)
    } else if catvod_firstaid::handles(site.api.trim()) {
        Ok(catvod_firstaid::categories())
    } else if catvod_nangua::handles(site.api.trim()) {
        Ok(catvod_nangua::categories())
    } else if catvod_libvio::handles(site.api.trim()) {
        catvod_libvio::categories(cfg, site)
    } else if catvod_jianpian::handles(site.api.trim()) {
        Ok(catvod_jianpian::categories())
    } else if catvod_jpys::handles(site.api.trim()) {
        Ok(catvod_jpys::categories())
    } else if catvod_yqkan::handles(site.api.trim()) {
        catvod_yqkan::categories(cfg, site)
    } else if catvod_apptt::handles(site.api.trim()) {
        Ok(catvod_apptt::categories())
    } else if catvod_appysv2::handles(site.api.trim()) {
        catvod_appysv2::categories(cfg, site)
    } else if catvod_ying::handles(site.api.trim()) {
        catvod_ying::categories(cfg, site)
    } else if catvod_kanqiu::handles(site.api.trim()) {
        Ok(catvod_kanqiu::categories())
    } else if catvod_kkys::handles(site.api.trim()) {
        catvod_kkys::categories(cfg, site)
    } else if catvod_mp4mov::handles(site.api.trim()) {
        Ok(catvod_mp4mov::categories())
    } else if catvod_zxzj::handles(site.api.trim()) {
        Ok(catvod_zxzj::categories(cfg, site)?)
    } else if catvod_duanju::handles(site.api.trim()) {
        Ok(catvod_duanju::categories())
    } else if catvod_xb6v::handles(site.api.trim()) {
        catvod_xb6v::categories(cfg, site)
    } else if catvod_meijumi::handles(site.api.trim()) {
        catvod_meijumi::categories(cfg, site)
    } else if catvod_ddrk::handles(site.api.trim()) {
        catvod_ddrk::categories(cfg, site)
    } else if catvod_auete::handles(site.api.trim()) {
        catvod_auete::categories(cfg, site)
    } else if catvod_iktv::handles(site.api.trim()) {
        Ok(catvod_iktv::categories())
    } else if catvod_yinyuetai::handles(site.api.trim()) {
        Ok(catvod_yinyuetai::categories())
    } else if catvod_alist::handles(site.api.trim()) {
        catvod_alist::categories(cfg, site)
    } else if catvod_tangrenjie::handles(site.api.trim()) {
        catvod_tangrenjie::categories(cfg, site)
    } else if catvod_quark::handles(site.api.trim()) {
        Ok(catvod_quark::categories())
    } else if catvod_share::handles(site.api.trim()) {
        catvod_share::categories(cfg, site)
    } else if catvod_moli::handles(site.api.trim()) {
        catvod_moli::categories(cfg, site)
    } else if catvod_xiaozhang::handles(site.api.trim()) {
        catvod_xiaozhang::categories(cfg, site)
    } else if catvod_ttian::handles(site.api.trim()) {
        catvod_ttian::categories(cfg, site)
    } else if catvod_wogg::handles(site.api.trim()) {
        catvod_wogg::categories(cfg, site)
    } else if catvod_xpanpan::handles(site.api.trim()) {
        catvod_xpanpan::categories(cfg, site)
    } else if catvod_appmao::handles(site.api.trim()) {
        catvod_appmao::categories(cfg, site)
    } else if catvod_appsk::handles(site.api.trim()) {
        catvod_appsk::categories(cfg, site)
    } else if catvod_bttwoo::handles(site.api.trim()) {
        catvod_bttwoo::categories(cfg, site)
    } else if catvod_xinsj::handles(site.api.trim()) {
        Ok(catvod_xinsj::categories(cfg, site))
    } else if catvod_seedhub::handles(site.api.trim()) {
        Ok(catvod_seedhub::categories(cfg, site))
    } else if catvod_xpath::handles(site.api.trim()) {
        Ok(catvod_xpath::categories(cfg, site)?)
    } else if catvod_nmys::handles(site.api.trim()) {
        Ok(catvod_nmys::categories(cfg, site)?)
    } else if catvod_nini::handles(site.api.trim()) {
        Ok(catvod_nini::categories(cfg, site)?)
    } else if catvod_bdys01::handles(site.api.trim()) {
        Ok(catvod_bdys01::categories(cfg, site)?)
    } else if catvod_czsapp::handles(site.api.trim()) {
        Ok(catvod_czsapp::categories(cfg, site)?)
    } else if catvod_gitcafe::handles(site.api.trim()) {
        Ok(catvod_gitcafe::categories(cfg, site)?)
    } else if catvod_bt0::handles(site.api.trim()) {
        Ok(catvod_bt0::categories(cfg, site)?)
    } else if catvod_hdhive::handles(site.api.trim()) {
        Ok(catvod_hdhive::categories(cfg, site)?)
    } else if catvod_xbiu::handles(site.api.trim()) {
        Ok(catvod_xbiu::categories(cfg, site)?)
    } else if catvod_zhaozy::handles(site.api.trim()) {
        Ok(catvod_zhaozy::categories(cfg, site)?)
    } else if catvod_gaoqing::handles(site.api.trim()) {
        Ok(catvod_gaoqing::categories(cfg, site)?)
    } else if catvod_panyq::handles(site.api.trim()) {
        Ok(catvod_panyq::categories(cfg, site)?)
    } else if catvod_pushshare::handles(site.api.trim()) {
        Ok(catvod_pushshare::categories(cfg, site)?)
    } else if catvod_p115share::handles(site.api.trim()) {
        Ok(catvod_p115share::categories(cfg, site)?)
    } else if catvod_pikpakshare::handles(site.api.trim()) {
        Ok(catvod_pikpakshare::categories(cfg, site)?)
    } else if catvod_youtube::handles(site.api.trim()) {
        catvod_youtube::categories(cfg, site)
    } else if catvod_sambashare::handles(site.api.trim()) {
        Ok(catvod_sambashare::categories(cfg, site)?)
    } else if catvod_thundershare::handles(site.api.trim()) {
        Ok(catvod_thundershare::categories(cfg, site)?)
    } else if catvod_tgyunpan::handles(site.api.trim()) {
        Ok(catvod_tgyunpan::categories(cfg, site)?)
    } else if catvod_alishare::handles(site.api.trim()) {
        Ok(catvod_alishare::categories(cfg, site)?)
    } else if catvod_kuaikan::handles(site.api.trim()) {
        Ok(catvod_kuaikan::categories(cfg, site)?)
    } else if catvod_xiaozhitiao::handles(site.api.trim()) {
        Ok(catvod_xiaozhitiao::categories(cfg, site)?)
    } else if catvod_liteapple::handles(site.api.trim()) {
        Ok(catvod_liteapple::categories(cfg, site))
    } else if catvod_yj1211::handles(site.api.trim()) {
        Ok(catvod_yj1211::categories(cfg, site))
    } else if catvod_push::handles(site.api.trim()) {
        Ok(catvod_push::categories(cfg, site))
    } else if catvod_fourkhdr::handles(site.api.trim()) {
        Ok(catvod_fourkhdr::categories(cfg, site))
    } else if catvod_yunpan::handles(site.api.trim()) {
        Ok(catvod_yunpan::categories(cfg, site))
    } else if catvod_appld::handles(site.api.trim()) {
        catvod_appld::categories(cfg, site)
    } else if catvod_yingso::handles(site.api.trim()) {
        Ok(catvod_yingso::categories(cfg, site))
    } else if catvod_huban::handles(site.api.trim()) {
        catvod_huban::categories(cfg, site)
    } else if catvod_tzfile::handles(site.api.trim()) {
        Ok(catvod_tzfile::categories())
    } else if catvod_sp360::handles(site.api.trim()) {
        Ok(catvod_sp360::categories())
    } else if catvod_kugou::handles(site.api.trim()) {
        Ok(catvod_kugou::categories())
    } else if catvod_kunyu77::handles(site.api.trim()) {
        Ok(catvod_kunyu77::categories())
    } else if catvod_douban::handles(site.api.trim()) {
        Ok(catvod_douban::categories())
    } else if catvod_appget::handles(site.api.trim()) {
        catvod_appget::categories(cfg, site)
    } else if catvod_pan::handles(site.api.trim()) {
        Ok(catvod_pan::categories())
    } else if catvod_pansearch::handles(site.api.trim()) {
        Ok(catvod_pansearch::categories())
    } else if api_is_js(site) {
        crate::spider::js::categories(site, cfg)
    } else if api_is_py(site) {
        crate::python::categories(cfg, site)
    } else {
        Err(crate::spider::catvod::site_error(site))
    }
}

fn list_videos_dispatch(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    if let Some(result) = crate::spider::catvod::descriptor::try_videos(cfg, site, cat, page) {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::list_videos(cfg, site, cat, page)
    } else if api_is_xyq(site) {
        crate::sources::xyq::list_videos(cfg, site, cat, page)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::list_videos(cfg, site, cat, page)
    } else if catvod_sixv::handles(site.api.trim()) {
        catvod_sixv::videos(cfg, site, cat, page)
    } else if catvod_ysj::handles(site.api.trim()) {
        catvod_ysj::videos(cfg, site, cat, page)
    } else if catvod_anime1::handles(site.api.trim()) {
        catvod_anime1::videos(cfg, site, cat, page)
    } else if catvod_dm84::handles(site.api.trim()) {
        catvod_dm84::videos(cfg, site, cat, page)
    } else if catvod_bili::handles(site.api.trim()) {
        catvod_bili::videos(cfg, site, cat, page)
    } else if catvod_firstaid::handles(site.api.trim()) {
        catvod_firstaid::videos(cfg, site, cat, page)
    } else if catvod_nangua::handles(site.api.trim()) {
        catvod_nangua::videos(cfg, site, cat, page)
    } else if catvod_libvio::handles(site.api.trim()) {
        catvod_libvio::videos(cfg, site, cat, page)
    } else if catvod_jianpian::handles(site.api.trim()) {
        catvod_jianpian::videos(cfg, site, cat, page)
    } else if catvod_jpys::handles(site.api.trim()) {
        catvod_jpys::videos(cfg, site, cat, page)
    } else if catvod_yqkan::handles(site.api.trim()) {
        catvod_yqkan::videos(cfg, site, cat, page)
    } else if catvod_apptt::handles(site.api.trim()) {
        catvod_apptt::videos(cfg, site, cat, page)
    } else if catvod_appysv2::handles(site.api.trim()) {
        catvod_appysv2::videos(cfg, site, cat, page)
    } else if catvod_ying::handles(site.api.trim()) {
        catvod_ying::videos(cfg, site, cat, page)
    } else if catvod_kanqiu::handles(site.api.trim()) {
        catvod_kanqiu::videos(cfg, site, cat, page)
    } else if catvod_kkys::handles(site.api.trim()) {
        catvod_kkys::videos(cfg, site, cat, page)
    } else if catvod_mp4mov::handles(site.api.trim()) {
        catvod_mp4mov::videos(cfg, site, cat, page)
    } else if catvod_zxzj::handles(site.api.trim()) {
        catvod_zxzj::videos(cfg, site, cat, page)
    } else if catvod_duanju::handles(site.api.trim()) {
        catvod_duanju::videos(cfg, site, cat, page)
    } else if catvod_xb6v::handles(site.api.trim()) {
        catvod_xb6v::videos(cfg, site, cat, page)
    } else if catvod_meijumi::handles(site.api.trim()) {
        catvod_meijumi::videos(cfg, site, cat, page)
    } else if catvod_ddrk::handles(site.api.trim()) {
        catvod_ddrk::videos(cfg, site, cat, page)
    } else if catvod_auete::handles(site.api.trim()) {
        catvod_auete::videos(cfg, site, cat, page)
    } else if catvod_iktv::handles(site.api.trim()) {
        catvod_iktv::videos(cfg, site, cat, page)
    } else if catvod_yinyuetai::handles(site.api.trim()) {
        catvod_yinyuetai::videos(cfg, site, cat, page)
    } else if catvod_alist::handles(site.api.trim()) {
        catvod_alist::videos(cfg, site, cat, page)
    } else if catvod_tangrenjie::handles(site.api.trim()) {
        catvod_tangrenjie::videos(cfg, site, cat, page)
    } else if catvod_quark::handles(site.api.trim()) {
        catvod_quark::videos(cfg, site, cat, page)
    } else if catvod_share::handles(site.api.trim()) {
        catvod_share::videos(cfg, site, cat, page)
    } else if catvod_moli::handles(site.api.trim()) {
        catvod_moli::videos(cfg, site, cat, page)
    } else if catvod_xiaozhang::handles(site.api.trim()) {
        catvod_xiaozhang::videos(cfg, site, cat, page)
    } else if catvod_ttian::handles(site.api.trim()) {
        catvod_ttian::videos(cfg, site, cat, page)
    } else if catvod_wogg::handles(site.api.trim()) {
        catvod_wogg::videos(cfg, site, cat, page)
    } else if catvod_xpanpan::handles(site.api.trim()) {
        catvod_xpanpan::videos(cfg, site, cat, page)
    } else if catvod_appmao::handles(site.api.trim()) {
        catvod_appmao::videos(cfg, site, cat, page)
    } else if catvod_appsk::handles(site.api.trim()) {
        catvod_appsk::videos(cfg, site, cat, page)
    } else if catvod_bttwoo::handles(site.api.trim()) {
        catvod_bttwoo::videos(cfg, site, cat, page)
    } else if catvod_xinsj::handles(site.api.trim()) {
        catvod_xinsj::videos(cfg, site, cat, page)
    } else if catvod_seedhub::handles(site.api.trim()) {
        catvod_seedhub::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_xpath::handles(site.api.trim()) {
        catvod_xpath::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_nmys::handles(site.api.trim()) {
        catvod_nmys::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_nini::handles(site.api.trim()) {
        catvod_nini::videos(cfg, site, cat, page)
    } else if catvod_bdys01::handles(site.api.trim()) {
        catvod_bdys01::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_czsapp::handles(site.api.trim()) {
        catvod_czsapp::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_gitcafe::handles(site.api.trim()) {
        catvod_gitcafe::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_bt0::handles(site.api.trim()) {
        catvod_bt0::videos(cfg, site, cat, page)
    } else if catvod_hdhive::handles(site.api.trim()) {
        catvod_hdhive::videos(cfg, site, cat, page)
    } else if catvod_xbiu::handles(site.api.trim()) {
        catvod_xbiu::videos(cfg, site, cat, page)
    } else if catvod_zhaozy::handles(site.api.trim()) {
        catvod_zhaozy::videos(cfg, site, cat, page)
    } else if catvod_gaoqing::handles(site.api.trim()) {
        catvod_gaoqing::videos(cfg, site, cat, page)
    } else if catvod_panyq::handles(site.api.trim()) {
        catvod_panyq::videos(cfg, site, cat, page)
    } else if catvod_pushshare::handles(site.api.trim()) {
        catvod_pushshare::videos(cfg, site, cat, page)
    } else if catvod_p115share::handles(site.api.trim()) {
        catvod_p115share::videos(cfg, site, cat, page)
    } else if catvod_pikpakshare::handles(site.api.trim()) {
        catvod_pikpakshare::videos(cfg, site, cat, page)
    } else if catvod_youtube::handles(site.api.trim()) {
        catvod_youtube::videos(cfg, site, cat, page)
    } else if catvod_sambashare::handles(site.api.trim()) {
        catvod_sambashare::videos(cfg, site, cat, page)
    } else if catvod_thundershare::handles(site.api.trim()) {
        catvod_thundershare::videos(cfg, site, cat, page)
    } else if catvod_tgyunpan::handles(site.api.trim()) {
        catvod_tgyunpan::videos(cfg, site, cat, page)
    } else if catvod_alishare::handles(site.api.trim()) {
        catvod_alishare::videos(cfg, site, cat, page)
    } else if catvod_kuaikan::handles(site.api.trim()) {
        catvod_kuaikan::videos(cfg, site, cat, page)
    } else if catvod_xiaozhitiao::handles(site.api.trim()) {
        catvod_xiaozhitiao::videos(cfg, site, cat, page)
    } else if catvod_liteapple::handles(site.api.trim()) {
        catvod_liteapple::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_yj1211::handles(site.api.trim()) {
        catvod_yj1211::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_push::handles(site.api.trim()) {
        catvod_push::videos(cfg, site, cat, page)
    } else if catvod_fourkhdr::handles(site.api.trim()) {
        catvod_fourkhdr::videos(cfg, site, cat, page)
    } else if catvod_yunpan::handles(site.api.trim()) {
        catvod_yunpan::videos(cfg, site, cat, page)
    } else if catvod_appld::handles(site.api.trim()) {
        catvod_appld::videos(cfg, site, cat, page)
    } else if catvod_yingso::handles(site.api.trim()) {
        catvod_yingso::videos(cfg, site, cat, page)
    } else if catvod_huban::handles(site.api.trim()) {
        catvod_huban::videos(cfg, site, cat, page)
    } else if catvod_tzfile::handles(site.api.trim()) {
        catvod_tzfile::videos(cfg, site, cat, page)
    } else if catvod_sp360::handles(site.api.trim()) {
        catvod_sp360::videos(cfg, site, cat, page)
    } else if catvod_kugou::handles(site.api.trim()) {
        catvod_kugou::videos(cfg, site, cat, page)
    } else if catvod_kunyu77::handles(site.api.trim()) {
        catvod_kunyu77::videos(cfg, site, cat, page)
    } else if catvod_douban::handles(site.api.trim()) {
        catvod_douban::videos(cfg, site, cat, page, &std::collections::HashMap::new())
    } else if catvod_appget::handles(site.api.trim()) {
        catvod_appget::videos(cfg, site, cat, page)
    } else if catvod_pan::handles(site.api.trim()) {
        catvod_pan::videos(cfg, site, cat, page)
    } else if catvod_pansearch::handles(site.api.trim()) {
        Ok(catvod_pansearch::videos())
    } else if api_is_js(site) {
        crate::spider::js::category(site, cfg, cat, page)
    } else if api_is_py(site) {
        crate::python::list_videos(cfg, site, cat, page)
    } else {
        Err(crate::spider::catvod::site_error(site))
    }
}

fn get_detail_dispatch(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    if let Some(result) = crate::spider::catvod::descriptor::try_detail(cfg, site, id) {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::get_detail(cfg, site, id)
    } else if api_is_xyq(site) {
        crate::sources::xyq::get_detail(cfg, site, id)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::get_detail(cfg, site, id)
    } else if catvod_sixv::handles(site.api.trim()) {
        catvod_sixv::detail(cfg, site, id)
    } else if catvod_ysj::handles(site.api.trim()) {
        catvod_ysj::detail(cfg, site, id)
    } else if catvod_anime1::handles(site.api.trim()) {
        catvod_anime1::detail(cfg, site, id)
    } else if catvod_dm84::handles(site.api.trim()) {
        catvod_dm84::detail(cfg, site, id)
    } else if catvod_bili::handles(site.api.trim()) {
        catvod_bili::detail(cfg, site, id)
    } else if catvod_firstaid::handles(site.api.trim()) {
        catvod_firstaid::detail(cfg, site, id)
    } else if catvod_nangua::handles(site.api.trim()) {
        catvod_nangua::detail(cfg, site, id)
    } else if catvod_libvio::handles(site.api.trim()) {
        catvod_libvio::detail(cfg, site, id)
    } else if catvod_jianpian::handles(site.api.trim()) {
        catvod_jianpian::detail(cfg, site, id)
    } else if catvod_jpys::handles(site.api.trim()) {
        catvod_jpys::detail(cfg, site, id)
    } else if catvod_yqkan::handles(site.api.trim()) {
        catvod_yqkan::detail(cfg, site, id)
    } else if catvod_apptt::handles(site.api.trim()) {
        catvod_apptt::detail(cfg, site, id)
    } else if catvod_appysv2::handles(site.api.trim()) {
        catvod_appysv2::detail(cfg, site, id)
    } else if catvod_ying::handles(site.api.trim()) {
        catvod_ying::detail(cfg, site, id)
    } else if catvod_kanqiu::handles(site.api.trim()) {
        catvod_kanqiu::detail(cfg, site, id)
    } else if catvod_kkys::handles(site.api.trim()) {
        catvod_kkys::detail(cfg, site, id)
    } else if catvod_mp4mov::handles(site.api.trim()) {
        catvod_mp4mov::detail(cfg, site, id)
    } else if catvod_zxzj::handles(site.api.trim()) {
        catvod_zxzj::detail(cfg, site, id)
    } else if catvod_duanju::handles(site.api.trim()) {
        catvod_duanju::detail(cfg, site, id)
    } else if catvod_xb6v::handles(site.api.trim()) {
        catvod_xb6v::detail(cfg, site, id)
    } else if catvod_meijumi::handles(site.api.trim()) {
        catvod_meijumi::detail(cfg, site, id)
    } else if catvod_ddrk::handles(site.api.trim()) {
        catvod_ddrk::detail(cfg, site, id)
    } else if catvod_auete::handles(site.api.trim()) {
        catvod_auete::detail(cfg, site, id)
    } else if catvod_iktv::handles(site.api.trim()) {
        catvod_iktv::detail(cfg, site, id)
    } else if catvod_yinyuetai::handles(site.api.trim()) {
        catvod_yinyuetai::detail(cfg, site, id)
    } else if catvod_alist::handles(site.api.trim()) {
        catvod_alist::detail(cfg, site, id)
    } else if catvod_tangrenjie::handles(site.api.trim()) {
        catvod_tangrenjie::detail(cfg, site, id)
    } else if catvod_quark::handles(site.api.trim()) {
        catvod_quark::detail(cfg, site, id)
    } else if catvod_share::handles(site.api.trim()) {
        catvod_share::detail(cfg, site, id)
    } else if catvod_moli::handles(site.api.trim()) {
        catvod_moli::detail(cfg, site, id)
    } else if catvod_xiaozhang::handles(site.api.trim()) {
        catvod_xiaozhang::detail(cfg, site, id)
    } else if catvod_ttian::handles(site.api.trim()) {
        catvod_ttian::detail(cfg, site, id)
    } else if catvod_wogg::handles(site.api.trim()) {
        catvod_wogg::detail(cfg, site, id)
    } else if catvod_xpanpan::handles(site.api.trim()) {
        catvod_xpanpan::detail(cfg, site, id)
    } else if catvod_appmao::handles(site.api.trim()) {
        catvod_appmao::detail(cfg, site, id)
    } else if catvod_appsk::handles(site.api.trim()) {
        catvod_appsk::detail(cfg, site, id)
    } else if catvod_bttwoo::handles(site.api.trim()) {
        catvod_bttwoo::detail(cfg, site, id)
    } else if catvod_xinsj::handles(site.api.trim()) {
        catvod_xinsj::detail(cfg, site, id)
    } else if catvod_seedhub::handles(site.api.trim()) {
        catvod_seedhub::detail(cfg, site, id)
    } else if catvod_xpath::handles(site.api.trim()) {
        catvod_xpath::detail(cfg, site, id)
    } else if catvod_nmys::handles(site.api.trim()) {
        catvod_nmys::detail(cfg, site, id)
    } else if catvod_nini::handles(site.api.trim()) {
        catvod_nini::detail(cfg, site, id)
    } else if catvod_bdys01::handles(site.api.trim()) {
        catvod_bdys01::detail(cfg, site, id)
    } else if catvod_czsapp::handles(site.api.trim()) {
        catvod_czsapp::detail(cfg, site, id)
    } else if catvod_gitcafe::handles(site.api.trim()) {
        catvod_gitcafe::detail(cfg, site, id)
    } else if catvod_bt0::handles(site.api.trim()) {
        catvod_bt0::detail(cfg, site, id)
    } else if catvod_hdhive::handles(site.api.trim()) {
        catvod_hdhive::detail(cfg, site, id)
    } else if catvod_xbiu::handles(site.api.trim()) {
        catvod_xbiu::detail(cfg, site, id)
    } else if catvod_zhaozy::handles(site.api.trim()) {
        catvod_zhaozy::detail(cfg, site, id)
    } else if catvod_gaoqing::handles(site.api.trim()) {
        catvod_gaoqing::detail(cfg, site, id)
    } else if catvod_panyq::handles(site.api.trim()) {
        catvod_panyq::detail(cfg, site, id)
    } else if catvod_pushshare::handles(site.api.trim()) {
        catvod_pushshare::detail(cfg, site, id)
    } else if catvod_p115share::handles(site.api.trim()) {
        catvod_p115share::detail(cfg, site, id)
    } else if catvod_pikpakshare::handles(site.api.trim()) {
        catvod_pikpakshare::detail(cfg, site, id)
    } else if catvod_youtube::handles(site.api.trim()) {
        catvod_youtube::detail(cfg, site, id)
    } else if catvod_sambashare::handles(site.api.trim()) {
        catvod_sambashare::detail(cfg, site, id)
    } else if catvod_thundershare::handles(site.api.trim()) {
        catvod_thundershare::detail(cfg, site, id)
    } else if catvod_tgyunpan::handles(site.api.trim()) {
        catvod_tgyunpan::detail(cfg, site, id)
    } else if catvod_alishare::handles(site.api.trim()) {
        catvod_alishare::detail(cfg, site, id)
    } else if catvod_kuaikan::handles(site.api.trim()) {
        catvod_kuaikan::detail(cfg, site, id)
    } else if catvod_xiaozhitiao::handles(site.api.trim()) {
        catvod_xiaozhitiao::detail(cfg, site, id)
    } else if catvod_liteapple::handles(site.api.trim()) {
        catvod_liteapple::detail(cfg, site, id)
    } else if catvod_yj1211::handles(site.api.trim()) {
        catvod_yj1211::detail(cfg, site, id)
    } else if catvod_push::handles(site.api.trim()) {
        catvod_push::detail(cfg, site, id)
    } else if catvod_fourkhdr::handles(site.api.trim()) {
        catvod_fourkhdr::detail(cfg, site, id)
    } else if catvod_yunpan::handles(site.api.trim()) {
        catvod_yunpan::detail(cfg, site, id)
    } else if catvod_appld::handles(site.api.trim()) {
        catvod_appld::detail(cfg, site, id)
    } else if catvod_yingso::handles(site.api.trim()) {
        catvod_yingso::detail(cfg, site, id)
    } else if catvod_huban::handles(site.api.trim()) {
        catvod_huban::detail(cfg, site, id)
    } else if catvod_tzfile::handles(site.api.trim()) {
        catvod_tzfile::detail(cfg, site, id)
    } else if catvod_sp360::handles(site.api.trim()) {
        catvod_sp360::detail(cfg, site, id)
    } else if catvod_kugou::handles(site.api.trim()) {
        catvod_kugou::detail(cfg, site, id)
    } else if catvod_kunyu77::handles(site.api.trim()) {
        catvod_kunyu77::detail(cfg, site, id)
    } else if catvod_douban::handles(site.api.trim()) {
        catvod_douban::detail(cfg, site, id)
    } else if catvod_appget::handles(site.api.trim()) {
        catvod_appget::detail(cfg, site, id)
    } else if catvod_pan::handles(site.api.trim()) {
        catvod_pan::detail(cfg, site, id)
    } else if catvod_pansearch::handles(site.api.trim()) {
        catvod_pansearch::detail(cfg, site, id)
    } else if api_is_js(site) {
        crate::spider::js::detail(site, cfg, id)
    } else if api_is_py(site) {
        crate::python::get_detail(cfg, site, id)
    } else {
        Err(crate::spider::catvod::site_error(site))
    }
}

fn search_dispatch(cfg: &TvConfig, site: &Site, keyword: &str, quick: bool) -> Result<Vec<VideoItem>> {
    if let Some(result) = crate::spider::catvod::descriptor::try_search(cfg, site, keyword) {
        return result;
    }
    if api_is_drpy(site) {
        crate::sources::drpy::search(cfg, site, keyword, quick)
    } else if api_is_xyq(site) {
        crate::sources::xyq::search(cfg, site, keyword, quick)
    } else if api_is_xbpq(site) {
        crate::sources::xbpq::search(cfg, site, keyword, quick)
    } else if catvod_sixv::handles(site.api.trim()) {
        catvod_sixv::search(cfg, site, keyword)
    } else if catvod_ysj::handles(site.api.trim()) {
        catvod_ysj::search(cfg, site, keyword)
    } else if catvod_anime1::handles(site.api.trim()) {
        catvod_anime1::search(cfg, site, keyword)
    } else if catvod_dm84::handles(site.api.trim()) {
        catvod_dm84::search(cfg, site, keyword)
    } else if catvod_bili::handles(site.api.trim()) {
        catvod_bili::search(cfg, site, keyword)
    } else if catvod_firstaid::handles(site.api.trim()) {
        catvod_firstaid::search(cfg, site, keyword)
    } else if catvod_nangua::handles(site.api.trim()) {
        catvod_nangua::search(cfg, site, keyword)
    } else if catvod_libvio::handles(site.api.trim()) {
        catvod_libvio::search(cfg, site, keyword)
    } else if catvod_jianpian::handles(site.api.trim()) {
        catvod_jianpian::search(cfg, site, keyword)
    } else if catvod_jpys::handles(site.api.trim()) {
        catvod_jpys::search(cfg, site, keyword)
    } else if catvod_yqkan::handles(site.api.trim()) {
        catvod_yqkan::search(cfg, site, keyword)
    } else if catvod_apptt::handles(site.api.trim()) {
        catvod_apptt::search(cfg, site, keyword)
    } else if catvod_appysv2::handles(site.api.trim()) {
        catvod_appysv2::search(cfg, site, keyword)
    } else if catvod_ying::handles(site.api.trim()) {
        catvod_ying::search(cfg, site, keyword)
    } else if catvod_kanqiu::handles(site.api.trim()) {
        catvod_kanqiu::search(cfg, site, keyword)
    } else if catvod_kkys::handles(site.api.trim()) {
        catvod_kkys::search(cfg, site, keyword)
    } else if catvod_mp4mov::handles(site.api.trim()) {
        catvod_mp4mov::search(cfg, site, keyword)
    } else if catvod_zxzj::handles(site.api.trim()) {
        catvod_zxzj::search(cfg, site, keyword)
    } else if catvod_duanju::handles(site.api.trim()) {
        catvod_duanju::search(cfg, site, keyword)
    } else if catvod_xb6v::handles(site.api.trim()) {
        catvod_xb6v::search(cfg, site, keyword)
    } else if catvod_meijumi::handles(site.api.trim()) {
        catvod_meijumi::search(cfg, site, keyword)
    } else if catvod_ddrk::handles(site.api.trim()) {
        catvod_ddrk::search(cfg, site, keyword)
    } else if catvod_auete::handles(site.api.trim()) {
        catvod_auete::search(cfg, site, keyword)
    } else if catvod_iktv::handles(site.api.trim()) {
        catvod_iktv::search(cfg, site, keyword)
    } else if catvod_yinyuetai::handles(site.api.trim()) {
        catvod_yinyuetai::search(cfg, site, keyword)
    } else if catvod_alist::handles(site.api.trim()) {
        catvod_alist::search(cfg, site, keyword)
    } else if catvod_tangrenjie::handles(site.api.trim()) {
        catvod_tangrenjie::search(cfg, site, keyword)
    } else if catvod_quark::handles(site.api.trim()) {
        catvod_quark::search(cfg, site, keyword)
    } else if catvod_share::handles(site.api.trim()) {
        catvod_share::search(cfg, site, keyword)
    } else if catvod_moli::handles(site.api.trim()) {
        catvod_moli::search(cfg, site, keyword)
    } else if catvod_xiaozhang::handles(site.api.trim()) {
        catvod_xiaozhang::search(cfg, site, keyword)
    } else if catvod_ttian::handles(site.api.trim()) {
        catvod_ttian::search(cfg, site, keyword)
    } else if catvod_wogg::handles(site.api.trim()) {
        catvod_wogg::search(cfg, site, keyword)
    } else if catvod_xpanpan::handles(site.api.trim()) {
        catvod_xpanpan::search(cfg, site, keyword)
    } else if catvod_appmao::handles(site.api.trim()) {
        catvod_appmao::search(cfg, site, keyword)
    } else if catvod_appsk::handles(site.api.trim()) {
        catvod_appsk::search(cfg, site, keyword)
    } else if catvod_bttwoo::handles(site.api.trim()) {
        catvod_bttwoo::search(cfg, site, keyword)
    } else if catvod_xinsj::handles(site.api.trim()) {
        catvod_xinsj::search(cfg, site, keyword)
    } else if catvod_seedhub::handles(site.api.trim()) {
        catvod_seedhub::search(cfg, site, keyword)
    } else if catvod_xpath::handles(site.api.trim()) {
        catvod_xpath::search(cfg, site, keyword)
    } else if catvod_nmys::handles(site.api.trim()) {
        catvod_nmys::search(cfg, site, keyword)
    } else if catvod_nini::handles(site.api.trim()) {
        catvod_nini::search(cfg, site, keyword)
    } else if catvod_bdys01::handles(site.api.trim()) {
        catvod_bdys01::search(cfg, site, keyword, quick)
    } else if catvod_czsapp::handles(site.api.trim()) {
        catvod_czsapp::search(cfg, site, keyword, true)
    } else if catvod_gitcafe::handles(site.api.trim()) {
        catvod_gitcafe::search(cfg, site, keyword)
    } else if catvod_bt0::handles(site.api.trim()) {
        catvod_bt0::search(cfg, site, keyword, false)
    } else if catvod_hdhive::handles(site.api.trim()) {
        catvod_hdhive::search(cfg, site, keyword, false)
    } else if catvod_xbiu::handles(site.api.trim()) {
        catvod_xbiu::search(cfg, site, keyword, false)
    } else if catvod_zhaozy::handles(site.api.trim()) {
        catvod_zhaozy::search(cfg, site, keyword, false)
    } else if catvod_gaoqing::handles(site.api.trim()) {
        catvod_gaoqing::search(cfg, site, keyword, false)
    } else if catvod_panyq::handles(site.api.trim()) {
        catvod_panyq::search(cfg, site, keyword)
    } else if catvod_pushshare::handles(site.api.trim()) {
        catvod_pushshare::search(cfg, site, keyword)
    } else if catvod_p115share::handles(site.api.trim()) {
        catvod_p115share::search(cfg, site, keyword)
    } else if catvod_pikpakshare::handles(site.api.trim()) {
        catvod_pikpakshare::search(cfg, site, keyword)
    } else if catvod_youtube::handles(site.api.trim()) {
        catvod_youtube::search(cfg, site, keyword)
    } else if catvod_sambashare::handles(site.api.trim()) {
        catvod_sambashare::search(cfg, site, keyword)
    } else if catvod_thundershare::handles(site.api.trim()) {
        catvod_thundershare::search(cfg, site, keyword)
    } else if catvod_tgyunpan::handles(site.api.trim()) {
        catvod_tgyunpan::search(cfg, site, keyword)
    } else if catvod_alishare::handles(site.api.trim()) {
        catvod_alishare::search(cfg, site, keyword)
    } else if catvod_kuaikan::handles(site.api.trim()) {
        catvod_kuaikan::search(cfg, site, keyword)
    } else if catvod_xiaozhitiao::handles(site.api.trim()) {
        catvod_xiaozhitiao::search(cfg, site, keyword)
    } else if catvod_liteapple::handles(site.api.trim()) {
        catvod_liteapple::search(cfg, site, keyword)
    } else if catvod_yj1211::handles(site.api.trim()) {
        catvod_yj1211::search(cfg, site, keyword)
    } else if catvod_push::handles(site.api.trim()) {
        catvod_push::search(cfg, site, keyword)
    } else if catvod_fourkhdr::handles(site.api.trim()) {
        catvod_fourkhdr::search(cfg, site, keyword)
    } else if catvod_yunpan::handles(site.api.trim()) {
        catvod_yunpan::search(cfg, site, keyword)
    } else if catvod_appld::handles(site.api.trim()) {
        catvod_appld::search(cfg, site, keyword)
    } else if catvod_yingso::handles(site.api.trim()) {
        catvod_yingso::search(cfg, site, keyword)
    } else if catvod_huban::handles(site.api.trim()) {
        catvod_huban::search(cfg, site, keyword)
    } else if catvod_tzfile::handles(site.api.trim()) {
        catvod_tzfile::search(cfg, site, keyword)
    } else if catvod_sp360::handles(site.api.trim()) {
        catvod_sp360::search(cfg, site, keyword)
    } else if catvod_kugou::handles(site.api.trim()) {
        catvod_kugou::search(cfg, site, keyword)
    } else if catvod_kunyu77::handles(site.api.trim()) {
        catvod_kunyu77::search(cfg, site, keyword)
    } else if catvod_douban::handles(site.api.trim()) {
        catvod_douban::search(cfg, site, keyword)
    } else if catvod_appget::handles(site.api.trim()) {
        catvod_appget::search(cfg, site, keyword)
    } else if catvod_pan::handles(site.api.trim()) {
        catvod_pan::search(cfg, site, keyword)
    } else if catvod_pansearch::handles(site.api.trim()) {
        catvod_pansearch::search(cfg, site, keyword)
    } else if api_is_js(site) {
        crate::spider::js::search(site, cfg, keyword, quick)
    } else if api_is_py(site) {
        crate::python::search(cfg, site, keyword, quick)
    } else {
        Err(crate::spider::catvod::site_error(site))
    }
}

/// 播放信息：type3 需要 spider.playerContent；桌面端 js / XYQHiker / 原生家族走原生实现
fn player_content_dispatch(
    cfg: &TvConfig,
    site: &Site,
    flag: &str,
    id: &str,
    vip_flags: &[String],
) -> Result<serde_json::Value> {
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
    } else if catvod_sixv::handles(site.api.trim()) {
        catvod_sixv::player_content(cfg, site, flag, id)
    } else if catvod_ysj::handles(site.api.trim()) {
        catvod_ysj::player_content(cfg, site, flag, id)
    } else if catvod_anime1::handles(site.api.trim()) {
        catvod_anime1::player_content(cfg, site, flag, id)
    } else if catvod_dm84::handles(site.api.trim()) {
        catvod_dm84::player_content(cfg, site, flag, id)
    } else if catvod_bili::handles(site.api.trim()) {
        catvod_bili::player_content(cfg, site, flag, id)
    } else if catvod_firstaid::handles(site.api.trim()) {
        catvod_firstaid::player_content(cfg, site, flag, id)
    } else if catvod_nangua::handles(site.api.trim()) {
        catvod_nangua::player_content(cfg, site, flag, id)
    } else if catvod_libvio::handles(site.api.trim()) {
        catvod_libvio::player_content(cfg, site, flag, id)

    } else if catvod_jianpian::handles(site.api.trim()) {
        catvod_jianpian::player_content(cfg, site, flag, id)
    } else if catvod_jpys::handles(site.api.trim()) {
        catvod_jpys::player_content(cfg, site, flag, id)
    } else if catvod_yqkan::handles(site.api.trim()) {
        catvod_yqkan::player_content(cfg, site, flag, id)
    } else if catvod_apptt::handles(site.api.trim()) {
        catvod_apptt::player_content(cfg, site, flag, id)
    } else if catvod_appysv2::handles(site.api.trim()) {
        catvod_appysv2::player_content(cfg, site, flag, id)
    } else if catvod_ying::handles(site.api.trim()) {
        catvod_ying::player_content(cfg, site, flag, id)
    } else if catvod_kanqiu::handles(site.api.trim()) {
        catvod_kanqiu::player_content(cfg, site, flag, id)
    } else if catvod_kkys::handles(site.api.trim()) {
        catvod_kkys::player_content(cfg, site, flag, id)
    } else if catvod_mp4mov::handles(site.api.trim()) {
        catvod_mp4mov::player_content(cfg, site, flag, id)
    } else if catvod_zxzj::handles(site.api.trim()) {
        catvod_zxzj::player_content(cfg, site, flag, id, vip_flags)
    } else if catvod_duanju::handles(site.api.trim()) {
        catvod_duanju::player_content(cfg, site, flag, id, vip_flags)
    } else if catvod_xb6v::handles(site.api.trim()) {
        catvod_xb6v::player_content(cfg, site, flag, id)
    } else if catvod_meijumi::handles(site.api.trim()) {
        catvod_meijumi::player_content(cfg, site, flag, id)
    } else if catvod_ddrk::handles(site.api.trim()) {
        catvod_ddrk::player_content(cfg, site, flag, id)
    } else if catvod_auete::handles(site.api.trim()) {
        catvod_auete::player_content(cfg, site, flag, id)
    } else if catvod_iktv::handles(site.api.trim()) {
        catvod_iktv::player_content(cfg, site, flag, id)
    } else if catvod_yinyuetai::handles(site.api.trim()) {
        catvod_yinyuetai::player_content(cfg, site, flag, id)
    } else if catvod_alist::handles(site.api.trim()) {
        catvod_alist::player_content(cfg, site, flag, id)
    } else if catvod_tangrenjie::handles(site.api.trim()) {
        catvod_tangrenjie::player_content(cfg, site, flag, id)
    } else if catvod_quark::handles(site.api.trim()) {
        catvod_quark::player_content(cfg, site, flag, id)
    } else if catvod_share::handles(site.api.trim()) {
        catvod_share::player_content(cfg, site, flag, id)
    } else if catvod_moli::handles(site.api.trim()) {
        catvod_moli::player_content(cfg, site, flag, id)
    } else if catvod_xiaozhang::handles(site.api.trim()) {
        catvod_xiaozhang::player_content(cfg, site, flag, id)
    } else if catvod_ttian::handles(site.api.trim()) {
        catvod_ttian::player_content(cfg, site, flag, id)
    } else if catvod_wogg::handles(site.api.trim()) {
        catvod_wogg::player_content(cfg, site, flag, id)
    } else if catvod_xpanpan::handles(site.api.trim()) {
        catvod_xpanpan::player_content(cfg, site, flag, id)
    } else if catvod_appmao::handles(site.api.trim()) {
        catvod_appmao::player_content(cfg, site, flag, id)
    } else if catvod_appsk::handles(site.api.trim()) {
        catvod_appsk::player_content(cfg, site, flag, id)
    } else if catvod_bttwoo::handles(site.api.trim()) {
        catvod_bttwoo::player_content(cfg, site, flag, id)
    } else if catvod_xinsj::handles(site.api.trim()) {
        catvod_xinsj::player_content(cfg, site, flag, id)
    } else if catvod_seedhub::handles(site.api.trim()) {
        catvod_seedhub::player_content(cfg, site, flag, id)
    } else if catvod_xpath::handles(site.api.trim()) {
        catvod_xpath::player_content(cfg, site, flag, id)
    } else if catvod_nmys::handles(site.api.trim()) {
        catvod_nmys::player_content(cfg, site, flag, id)
    } else if catvod_nini::handles(site.api.trim()) {
        catvod_nini::player_content(cfg, site, flag, id)
    } else if catvod_bdys01::handles(site.api.trim()) {
        catvod_bdys01::player_content(cfg, site, flag, id)
    } else if catvod_czsapp::handles(site.api.trim()) {
        catvod_czsapp::player_content(cfg, site, flag, id)
    } else if catvod_gitcafe::handles(site.api.trim()) {
        catvod_gitcafe::player_content(cfg, site, flag, id)
    } else if catvod_bt0::handles(site.api.trim()) {
        catvod_bt0::player_content(cfg, site, flag, id)
    } else if catvod_hdhive::handles(site.api.trim()) {
        catvod_hdhive::player_content(cfg, site, flag, id)
    } else if catvod_xbiu::handles(site.api.trim()) {
        catvod_xbiu::player_content(cfg, site, flag, id)
    } else if catvod_zhaozy::handles(site.api.trim()) {
        catvod_zhaozy::player_content(cfg, site, flag, id)
    } else if catvod_gaoqing::handles(site.api.trim()) {
        catvod_gaoqing::player_content(cfg, site, flag, id)
    } else if catvod_panyq::handles(site.api.trim()) {
        catvod_panyq::player_content(cfg, site, flag, id)
    } else if catvod_pushshare::handles(site.api.trim()) {
        catvod_pushshare::player_content(cfg, site, flag, id)
    } else if catvod_p115share::handles(site.api.trim()) {
        catvod_p115share::player_content(cfg, site, flag, id)
    } else if catvod_pikpakshare::handles(site.api.trim()) {
        catvod_pikpakshare::player_content(cfg, site, flag, id)
    } else if catvod_youtube::handles(site.api.trim()) {
        catvod_youtube::player_content(cfg, site, flag, id)
    } else if catvod_sambashare::handles(site.api.trim()) {
        catvod_sambashare::player_content(cfg, site, flag, id)
    } else if catvod_thundershare::handles(site.api.trim()) {
        catvod_thundershare::player_content(cfg, site, flag, id)
    } else if catvod_tgyunpan::handles(site.api.trim()) {
        catvod_tgyunpan::player_content(cfg, site, flag, id)
    } else if catvod_alishare::handles(site.api.trim()) {
        catvod_alishare::player_content(cfg, site, flag, id)
    } else if catvod_kuaikan::handles(site.api.trim()) {
        catvod_kuaikan::player_content(cfg, site, flag, id)
    } else if catvod_xiaozhitiao::handles(site.api.trim()) {
        catvod_xiaozhitiao::player_content(cfg, site, flag, id)
    } else if catvod_liteapple::handles(site.api.trim()) {
        catvod_liteapple::player_content(cfg, site, flag, id)
    } else if catvod_yj1211::handles(site.api.trim()) {
        catvod_yj1211::player_content(cfg, site, flag, id)
    } else if catvod_push::handles(site.api.trim()) {
        catvod_push::player_content(cfg, site, flag, id)
    } else if catvod_fourkhdr::handles(site.api.trim()) {
        catvod_fourkhdr::player_content(cfg, site, flag, id)
    } else if catvod_yunpan::handles(site.api.trim()) {
        catvod_yunpan::player_content(cfg, site, flag, id)
    } else if catvod_appld::handles(site.api.trim()) {
        catvod_appld::player_content(cfg, site, flag, id)
    } else if catvod_yingso::handles(site.api.trim()) {
        catvod_yingso::player_content(cfg, site, flag, id)
    } else if catvod_huban::handles(site.api.trim()) {
        catvod_huban::player_content(cfg, site, flag, id)
    } else if catvod_tzfile::handles(site.api.trim()) {
        catvod_tzfile::player_content(cfg, site, flag, id)
    } else if catvod_sp360::handles(site.api.trim()) {
        catvod_sp360::player_content(cfg, site, flag, id)
    } else if catvod_kugou::handles(site.api.trim()) {
        catvod_kugou::player_content(cfg, site, flag, id, vip_flags)
    } else if catvod_kunyu77::handles(site.api.trim()) {
        catvod_kunyu77::player_content(cfg, site, flag, id)
    } else if catvod_douban::handles(site.api.trim()) {
        catvod_douban::player_content(cfg, site, flag, id)
    } else if catvod_appget::handles(site.api.trim()) {
        catvod_appget::player_content(cfg, site, flag, id)
    } else if catvod_pan::handles(site.api.trim()) {
        catvod_pan::player_content(cfg, site, flag, id)
    } else if catvod_pansearch::handles(site.api.trim()) {
        catvod_pansearch::player_content(cfg, site)
    } else if api_is_js(site) {
        crate::spider::js::player_content(site, cfg, flag, id, vip_flags)
    } else if api_is_py(site) {
        crate::python::player_content(cfg, site, flag, id, vip_flags)
    } else {
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
