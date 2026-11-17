//! catvod 家族原生实现（jar → Rust 垂直切片）。
//!
//! 一段家族实现 = 「站点级输入（ext/api）→ 请求 → 解析 → VBox 强类型」，
//! 不依赖 dex、不依赖 Android Host。每移植一个家族，就在 `handles` +
//! `sources::spider` 的分派里加一条分支，并把 `profiles.json` 的 strategy
//! 从 `unported` 改成 `rulepack`（reason=`existing_native_implementation`）。

pub mod alist;
pub mod anime1;
pub mod appget;
pub mod apptt;
pub mod appysv2;
pub mod appmao;
pub mod appld;
pub mod appsk;
pub mod bttwoo;
pub mod yunpan;
pub mod fourkhdr;
pub mod auete;
pub mod bili;
pub mod dm84;
pub mod ddrk;
pub mod douban;
pub mod duanju;
pub mod firstaid;
pub mod huban;
pub mod jianpian;
pub mod jpys;
pub mod iktv;
pub mod kugou;
pub mod kunyu77;
pub mod kanqiu;
pub mod kkys;
pub mod libvio;
pub mod meijumi;
pub mod moli;
pub mod mp4mov;
pub mod nangua;
pub mod pan;
pub mod panyq;
pub mod pansearch;
pub mod quark;
pub mod sixv;
pub mod sp360;
pub mod tangrenjie;
pub mod xb6v;
pub mod youtube;
pub mod xiaozhang;
pub mod ying;
pub mod yinyuetai;
pub mod yqkan;
pub mod ysj;
pub mod zxzj;
pub mod tzfile;
pub mod ttian;
pub mod wogg;
pub mod xpanpan;
pub mod yingso;
pub mod share;
pub mod push;
pub mod seedhub;
pub mod xinsj;
pub mod yj1211;
pub mod liteapple;
pub mod xpath;
pub mod nmys;
pub mod nini;
pub mod bdys01;
pub mod xiaozhitiao;
pub mod czsapp;
pub mod gitcafe;
pub mod kuaikan;
pub mod alishare;
pub mod p115share;
pub mod sambashare;
pub mod pikpakshare;
pub mod thundershare;
pub mod tgyunpan;
pub mod bt0;
pub mod gaoqing;
pub mod zhaozy;
pub mod hdhive;
pub mod xbiu;
pub mod pushshare;

/// 返回该 api 对应的原生家族名（用于错误信息/观测）。
pub fn family_of(api: &str) -> Option<&'static str> {
    let api = api.trim();
    if xinsj::handles(api) {
        Some("xinsj")
    } else if seedhub::handles(api) {
        Some("seedhub")
    } else if xpath::handles(api) {
        Some("xpath")
    } else if nmys::handles(api) {
        Some("nmys")
    } else if nini::handles(api) {
        Some("nini")
    } else if bdys01::handles(api) {
        Some("bdys01")
    } else if czsapp::handles(api) {
        Some("czsapp")
    } else if gitcafe::handles(api) {
        Some("gitcafe")
    } else if alishare::handles(api) {
        Some("alishare")
    } else if zhaozy::handles(api) {
        Some("zhaozy")
    } else if gaoqing::handles(api) {
        Some("gaoqing")
    } else if bt0::handles(api) {
        Some("bt0")
    } else if kuaikan::handles(api) {
        Some("kuaikan")
    } else if xiaozhitiao::handles(api) {
        Some("xiaozhitiao")
    } else if liteapple::handles(api) {
        Some("liteapple")
    } else if yj1211::handles(api) {
        Some("yj1211")
    } else if hdhive::handles(api) {
        Some("hdhive")
    } else if xbiu::handles(api) {
        Some("xbiu")
    } else if push::handles(api) {
        Some("push")
    } else if alist::handles(api) {
        Some("alist")
    } else if appget::handles(api) {
        Some("appget")
    } else if pansearch::handles(api) {
        Some("pansearch")
    } else if sixv::handles(api) {
        Some("sixv")
    } else if ysj::handles(api) {
        Some("ysj")
    } else if anime1::handles(api) {
        Some("anime1")
    } else if dm84::handles(api) {
        Some("dm84")
    } else if firstaid::handles(api) {
        Some("firstaid")
    } else if ddrk::handles(api) {
        Some("ddrk")
    } else if auete::handles(api) {
        Some("auete")
    } else if huban::handles(api) {
        Some("huban")
    } else if douban::handles(api) {
        Some("douban")
    } else if xiaozhang::handles(api) {
        Some("xiaozhang")
    } else if bili::handles(api) {
        Some("bili")
    } else if nangua::handles(api) {
        Some("nangua")
    } else if jianpian::handles(api) {
        Some("jianpian")
    } else if jpys::handles(api) {
        Some("jpys")
    } else if libvio::handles(api) {
        Some("libvio")
    } else if meijumi::handles(api) {
        Some("meijumi")
    } else if moli::handles(api) {
        Some("moli")
    } else if tzfile::handles(api) {
        Some("tzfile")
    } else if ttian::handles(api) {
        Some("ttian")
    } else if wogg::handles(api) {
        Some("wogg")
    } else if xpanpan::handles(api) {
        Some("xpanpan")
    } else if yingso::handles(api) {
        Some("yingso")
    } else if iktv::handles(api) {
        Some("iktv")
    } else if ying::handles(api) {
        Some("ying")
    } else if yinyuetai::handles(api) {
        Some("yinyuetai")
    } else if yqkan::handles(api) {
        Some("yqkan")
    } else if apptt::handles(api) {
        Some("apptt")
    } else if appysv2::handles(api) {
        Some("appysv2")
    } else if appmao::handles(api) {
        Some("appmao")
    } else if appsk::handles(api) {
        Some("appsk")
    } else if bttwoo::handles(api) {
        Some("bttwoo")
    } else if fourkhdr::handles(api) {
        Some("fourkhdr")
    } else if yunpan::handles(api) {
        Some("yunpan")
    } else if appld::handles(api) {
        Some("appld")
    } else if kugou::handles(api) {
        Some("kugou")
    } else if kanqiu::handles(api) {
        Some("kanqiu")
    } else if kkys::handles(api) {
        Some("kkys")
    } else if kunyu77::handles(api) {
        Some("kunyu77")
    } else if mp4mov::handles(api) {
        Some("mp4mov")
    } else if duanju::handles(api) {
        Some("duanju")
    } else if zxzj::handles(api) {
        Some("zxzj")
    } else if xb6v::handles(api) {
        Some("xb6v")
    } else if sp360::handles(api) {
        Some("sp360")
    } else if tangrenjie::handles(api) {
        Some("tangrenjie")
    } else if youtube::handles(api) {
        Some("youtube")
    } else if sambashare::handles(api) {
            Some("sambashare")
        } else if pushshare::handles(api) {
        Some("pushshare")
        } else if p115share::handles(api) {
        Some("p115share")
    } else if pikpakshare::handles(api) {
        Some("pikpakshare")
    } else if thundershare::handles(api) {
        Some("thundershare")
    } else if tgyunpan::handles(api) {
        Some("tgyunpan")
    } else if panyq::handles(api) {
        Some("panyq")
    } else if pan::handles(api) {
        Some("pan")
    } else if quark::handles(api) {
        Some("quark")
    } else if share::handles(api) {
        Some("share")
    } else {
        None
    }
}
