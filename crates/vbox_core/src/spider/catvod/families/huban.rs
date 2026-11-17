//! `csp_Huban`（弹幕|小窗）原生实现 —— dc 仓 `https://szyyds.cn/tv/x.json`，1 个站点。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/02583298-HubanTC/deobf/com/github/catvod/spider/Huban.java`（99 行，
//!   jar `_rea/samples/library/jars/02583298-HubanTC.jar`）；
//! * 类内唯一字符串表 `f4short`（91 个 short）+ `C0003.m538(arr,start,len,key)` 系列
//!   helper；解码规则为 `char = (shortArr[start+i] ^ key)`（按 len 截取），
//!   已用 `_rea/tmp/huban-dec.mjs` 逐条验证（10/10 条全部解出，见 plan §122 / REA 附录 DW）。
//!
//! 这是一个「配置引导 / 动作」壳站：不提供影视内容，只在内置分类里给出一条
//! 「弹幕弹窗设置」动作条目，点进详情显示 `配置中心` 线路，播放时提示「请配置参数」。
//!
//! 协议（原类逐方法映射）：
//! * `homeContent(z)` → `{"class":[{"type_id":"1","type_name":"小虎斑的口粮"}],"list":[]}`
//!   （分类由 `E.b` 构造，无 filters）；
//! * `categoryContent(typeId, page, …)` → 仅当 `page == "1"` 时返回一条动作条目
//!   （`E.j(int)`：`vod_id = vod_name = action = "弹幕弹窗设置"`，`vod_pic/vod_remarks = ""`），
//!   其余页返回空 `list`；
//! * `detailContent([id])` → `vod_id = vod_name = id`、`vod_pic = ""`、
//!   `vod_remarks = "配置设置"`、`vod_play_from = "配置中心"`、
//!   `vod_play_url = "配置$" + id`；
//! * `playerContent(flag, id)` → 固定 `{"parse":0,"url":"","msg":"请配置参数","errMsg":"请配置参数"}`；
//! * `searchContent(str, z)` → 原类先弹 toast「虎：获取到剧名啦」并记录关键词，然后返回空
//!   `{"list":[]}`（`E.e.h(list)`）。
//!
//! 有意偏差（桌面端能力/契约取舍）：
//! 1. `action()`（原类点击动作条目后打开 Android 弹幕设置对话框）不实现 —— 桌面端没有
//!    对应的 `Spider.action` 入口，条目仍展示，点击由宿主忽略；
//! 2. `searchContent` 的 toast 不实现（只保留「空结果」语义）；
//! 3. 原类 `init(Context)` 为空，没有 ext / 网络 / Cookie，因此本家族完全不发请求。
//!
//! 现站核验（2026-11）：站点 `ext` 为 `null`、类内无基址常量，原类不访问任何服务器，
//! 因此无需真站用例（纯离线，可 100% 等价 mock）。

use serde_json::{json, Value};

use super::super::contract::{CatvodError, ErrorClass};
use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

/// 订阅配置里的 api 名。
pub const API: &str = "csp_Huban";

/// `homeContent` 的唯一分类（解码自 `f4short[18..19]`）。
const CATEGORY_ID: &str = "1";
const CATEGORY_NAME: &str = "小虎斑的口粮";

/// 动作条目的 id / 名称（解码自 `E.j.f20short` + key 1799，长度 6）。
const ACTION_TITLE: &str = "弹幕弹窗设置";

/// `detailContent` 的备注与线路名（解码自 `f4short[7..11]` / `[11..15]`）。
const DETAIL_REMARK: &str = "配置设置";
const DETAIL_PLAY_FROM: &str = "配置中心";

/// `detailContent` 的播放地址前缀（解码自 `f4short[15..18]`）。
const PLAY_URL_PREFIX: &str = "配置$";

/// `playerContent` 的固定提示（解码自 `f4short[25..76]`）。
const CONFIG_MSG: &str = "请配置参数";

/// 是否是本家族接管的 api（大小写不敏感；字面量集中在这里，供
/// `site-coverage` / `live-coverage` 静态扫描收集家族 api）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// 分类（`homeContent` 的 `class` 部分）。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(vec![Category {
        type_id: CATEGORY_ID.to_string(),
        type_name: CATEGORY_NAME.to_string(),
    }])
}

/// 列表（`categoryContent`）：仅第 1 页返回「弹幕弹窗设置」动作条目。
pub fn videos(
    _cfg: &TvConfig,
    _site: &Site,
    _type_id: &str,
    page: u32,
) -> Result<Vec<VideoItem>> {
    if page != 1 {
        return Ok(Vec::new());
    }
    Ok(vec![VideoItem {
        id: ACTION_TITLE.to_string(),
        name: ACTION_TITLE.to_string(),
        cover: None,
        remark: None,
    }])
}

/// 详情（`detailContent`）：把 `vod_play_from/vod_play_url` 映射成一条线路。
pub fn detail(_cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "Huban 详情 id 为空"));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name: id.to_string(),
        cover: None,
        desc: None,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: DETAIL_PLAY_FROM.to_string(),
            items: vec![Episode {
                title: id.to_string(),
                url: format!("{PLAY_URL_PREFIX}{id}"),
            }],
        }],
    })
}

/// 搜索（`searchContent`）：原类只弹 toast，结果恒为空。
pub fn search(_cfg: &TvConfig, _site: &Site, _keyword: &str) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// 播放（`playerContent`）：固定返回「请配置参数」提示，不含直链。
pub fn player_content(
    _cfg: &TvConfig,
    _site: &Site,
    _flag: &str,
    _id: &str,
) -> Result<Value> {
    Ok(json!({
        "parse": 0,
        "url": "",
        "msg": CONFIG_MSG,
        "errMsg": CONFIG_MSG,
    }))
}

/// 详情备注（导出给集成测试/宿主提示用）。
pub fn detail_remark() -> &'static str {
    DETAIL_REMARK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            key: "huban".to_string(),
            name: "弹幕|小窗".to_string(),
            api: API.to_string(),
            ..Default::default()
        }
    }

    fn cfg() -> TvConfig {
        TvConfig::default()
    }

    #[test]
    fn api_constant_matches_subscription() {
        assert_eq!(API, "csp_Huban");
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Huban"));
        assert!(handles("CSP_HUBAN"));
        assert!(handles(" csp_huban "));
        assert!(!handles("csp_Huban2"));
        assert!(!handles(""));
    }

    #[test]
    fn categories_has_single_guiding_entry() {
        let cats = categories(&cfg(), &site()).expect("categories");
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "小虎斑的口粮");
    }

    #[test]
    fn first_page_has_action_entry_and_others_are_empty() {
        let items = videos(&cfg(), &site(), "1", 1).expect("videos");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "弹幕弹窗设置");
        assert_eq!(items[0].name, "弹幕弹窗设置");
        assert!(items[0].cover.is_none());
        assert!(items[0].remark.is_none());

        for page in [2u32, 3, 9] {
            assert!(videos(&cfg(), &site(), "1", page).expect("videos").is_empty());
        }
    }

    #[test]
    fn detail_maps_play_from_and_config_url() {
        let d = detail(&cfg(), &site(), "弹幕弹窗设置").expect("detail");
        assert_eq!(d.id, "弹幕弹窗设置");
        assert_eq!(d.name, "弹幕弹窗设置");
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "配置中心");
        assert_eq!(d.episodes[0].items.len(), 1);
        assert_eq!(d.episodes[0].items[0].title, "弹幕弹窗设置");
        assert_eq!(d.episodes[0].items[0].url, "配置$弹幕弹窗设置");
        assert_eq!(detail_remark(), "配置设置");
    }

    #[test]
    fn detail_rejects_blank_id() {
        let err = detail(&cfg(), &site(), "   ").expect_err("blank id must fail");
        let text = format!("{err}");
        assert!(text.contains("id 为空"), "unexpected error: {text}");
    }

    #[test]
    fn search_is_always_empty() {
        for kw in ["", "沙丘", "弹幕"] {
            assert!(search(&cfg(), &site(), kw).expect("search").is_empty());
        }
    }

    #[test]
    fn player_returns_config_hint_without_url() {
        let v = player_content(&cfg(), &site(), "配置中心", "弹幕弹窗设置").expect("player");
        assert_eq!(v["parse"], 0);
        assert_eq!(v["url"], "");
        assert_eq!(v["msg"], "请配置参数");
        assert_eq!(v["errMsg"], "请配置参数");
    }
}
