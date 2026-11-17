//! 判据 77：五个分发入口必须与 `families/mod.rs` 的家族清单一一对应。
//!
//! 为什么需要这一条：新增一个原生家族要同时改 `sources/spider.rs` 里**五条彼此独立**的
//! if-else 链（分类/列表/详情/搜索/播放）。漏改其中一条不会编译失败，只会让该 api 在
//! 那个入口静默落到 js/py 启发式或 `site_error`——历史上还发生过批量插入分支导致文件
//! 膨胀的事故。另外族分支必须排在 js/py 启发式**之前**：jar 源的 ext 常常就是一个
//! 站点 URL，`python::is_python_site` 会把「ext 是 http 地址」误判成 py 源。
//!
//! 本判据只做静态结构校验，不触网、不构造 Site：
//! 1. 每个 `pub mod <m>;` 都有 `use ... as catvod_<m>;`，且 `catvod_<m>::handles(` 恰 5 次；
//! 2. 五个 dispatch 函数体各自覆盖全部家族（臂数 == 模块数），且每个族臂位置都早于
//!    该函数里第一个 `api_is_js(` / `api_is_py(`。

use std::fs;
use std::path::PathBuf;

const DISPATCH_FNS: [&str; 5] = [
    "list_categories_dispatch",
    "list_videos_dispatch",
    "get_detail_dispatch",
    "search_dispatch",
    "player_content_dispatch",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {rel} 失败：{e}"))
}

/// 取一个顶层 `fn <name>(` 的函数体，切到下一个行首 `fn ` 为止。
fn dispatch_body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("找不到分发函数 {name}"));
    let rest = &src[start..];
    let end = rest[1..]
        .find("\nfn ")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn every_family_module_has_import_and_five_dispatch_arms() {
    let mods_src = read("crates/vbox_core/src/spider/catvod/families/mod.rs");
    let spider = read("crates/vbox_core/src/sources/spider.rs");

    let re_mod = regex::Regex::new(r"(?m)^pub mod (\w+);").unwrap();
    let modules: Vec<&str> = re_mod
        .captures_iter(&mods_src)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    assert!(
        modules.len() >= 60,
        "家族模块只有 {} 个，parse 或 mod.rs 可疑",
        modules.len()
    );

    let mut missing_import = Vec::new();
    let mut bad_arms = Vec::new();
    for m in &modules {
        let import = format!("use crate::spider::catvod::families::{m} as catvod_{m};");
        if !spider.contains(&import) {
            missing_import.push(*m);
        }
        let needle = format!("catvod_{m}::handles(");
        let n = spider.matches(&needle).count();
        if n != DISPATCH_FNS.len() {
            bad_arms.push(format!("{m}:{n}"));
        }
    }
    assert!(missing_import.is_empty(), "缺少家族 import：{missing_import:?}");
    assert!(bad_arms.is_empty(), "分派臂数不为 5：{bad_arms:?}");

    let re_arm = regex::Regex::new(r"catvod_\w+::handles\(").unwrap();
    for f in DISPATCH_FNS {
        let body = dispatch_body(&spider, f);
        let arms = re_arm.find_iter(body).count();
        assert_eq!(
            arms,
            modules.len(),
            "{f} 的家族臂数 {arms} != 模块数 {}",
            modules.len()
        );
    }
}

#[test]
fn family_arms_precede_js_py_heuristics_in_all_dispatch_chains() {
    let spider = read("crates/vbox_core/src/sources/spider.rs");
    let re_arm = regex::Regex::new(r"catvod_\w+::handles\(").unwrap();

    for f in DISPATCH_FNS {
        let body = dispatch_body(&spider, f);
        let js = body
            .find("api_is_js(")
            .unwrap_or_else(|| panic!("{f} 缺少 api_is_js 兜底"));
        let py = body
            .find("api_is_py(")
            .unwrap_or_else(|| panic!("{f} 缺少 api_is_py 兜底"));
        let first_heuristic = js.min(py);
        let arms: Vec<usize> = re_arm.find_iter(body).map(|m| m.start()).collect();
        assert!(!arms.is_empty(), "{f} 没有任何家族臂");
        let late: Vec<usize> = arms
            .iter()
            .copied()
            .filter(|&p| p > first_heuristic)
            .collect();
        assert!(
            late.is_empty(),
            "{f} 有 {} 个族臂落在 js/py 启发式之后（位置 {:?} > {}）",
            late.len(),
            late,
            first_heuristic
        );
    }
}

/// 从单个家族文件的 `pub fn handles` 体里取它声明的 api：
/// 直接字面量 + 体内引用的本文件 `const NAME: &str = "csp_…"`。
fn family_handles_apis(src: &str) -> Vec<String> {
    let start = src
        .find("pub fn handles")
        .unwrap_or_else(|| panic!("家族文件缺少 handles"));
    let body_start = src[start..]
        .find('{')
        .map(|i| start + i)
        .unwrap_or_else(|| panic!("handles 缺少函数体"));
    let mut depth = 0usize;
    let mut end = body_start;
    for (i, ch) in src[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &src[body_start..=end];
    let re_lit = regex::Regex::new(r#""(csp_[A-Za-z0-9_]+)""#).unwrap();
    let mut out: Vec<String> = re_lit
        .captures_iter(body)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .collect();
    let re_const = regex::Regex::new(r#"const ([A-Z0-9_]+): &str = "(csp_[A-Za-z0-9_]+)""#).unwrap();
    for c in re_const.captures_iter(src) {
        let name = c.get(1).unwrap().as_str();
        if body.contains(name) {
            out.push(c.get(2).unwrap().as_str().to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn family_handles_match_rulepack_and_never_claim_blocked_api() {
    // 判据 77 的前两项只保证「模块 ↔ 入口」结构完整，但分派器认哪些 api
    // 是由各家族 `handles` 的**字符串集合**决定的，画像只是报告侧声明。
    // 两边任何一处滞后都是真实不一致：
    // - 家族已接管、画像仍标 unported/unsupported → 报告说「阻断」，实际能跑；
    // - 家族仍声明、画像已标 blocked → 分派器把已阻断 api 当原生实现；
    // - 新增家族漏登记画像 → 判据 62 的徽章检查也未必覆盖（徽章是另一张名单）。
    // 因此这里做双向集合对账，引擎 api（规则引擎，不是家族）单独豁免。
    const ENGINE_APIS: [&str; 3] = ["csp_XBPQ", "csp_XYQHiker", "csp_XYQHikerAL"];
    let dir = workspace_root().join("crates/vbox_core/src/spider/catvod/families");
    let mods_src = read("crates/vbox_core/src/spider/catvod/families/mod.rs");
    let re_mod = regex::Regex::new(r"(?m)^pub mod (\w+);").unwrap();
    let modules: Vec<String> = re_mod
        .captures_iter(&mods_src)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .collect();

    let profiles: serde_json::Value = serde_json::from_str(&read(
        "crates/vbox_core/src/spider/catvod/profiles.json",
    ))
    .expect("profiles.json 不是合法 JSON");
    let apis = profiles["apis"].as_object().expect("profiles.apis 缺失");
    let mut lower_to_key = std::collections::BTreeMap::new();
    for key in apis.keys() {
        lower_to_key.insert(key.to_ascii_lowercase(), key.clone());
    }

    let mut claims: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut duplicate_claims = Vec::new();
    for m in &modules {
        let path = dir.join(format!("{m}.rs"));
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {m}.rs 失败：{e}"));
        let handles = family_handles_apis(&src);
        for api in handles {
            let lower = api.to_ascii_lowercase();
            match claims.get(&lower) {
                Some(prev) if prev == m => continue, // 同一模块内的拼写变体（如 csp_Wogg/csp_WoGG）
                Some(prev) => duplicate_claims.push(format!("{api} 同时被 {prev} 与 {m} 声明")),
                None => {
                    claims.insert(lower.clone(), m.clone());
                }
            }
            let Some(key) = lower_to_key.get(&lower) else {
                panic!("家族 {m} 声明了画像中不存在的 api：{api}");
            };
            let strategy = apis[key]["strategy"].as_str().unwrap_or("");
            assert_eq!(
                strategy, "rulepack",
                "家族 {m} 声明 {api}，但画像策略是 {strategy}（分派器与报告不一致）"
            );
        }
    }
    assert!(
        duplicate_claims.is_empty(),
        "同一 api 被多个家族声明：{duplicate_claims:?}"
    );

    let mut expected: Vec<String> = apis
        .iter()
        .filter(|(_, v)| v["strategy"].as_str() == Some("rulepack"))
        .map(|(k, _)| k.to_ascii_lowercase())
        .filter(|k| !ENGINE_APIS.iter().any(|e| e.to_ascii_lowercase() == *k))
        .collect();
    expected.sort();
    expected.dedup();
    let actual: Vec<String> = claims.keys().cloned().collect();
    assert_eq!(
        actual, expected,
        "家族 handles 集合与 rulepack（剔除 3 个引擎）不一致；actual={actual:?} expected={expected:?}"
    );
}

#[test]
fn dispatch_tolerates_surrounding_api_whitespace() {
    use vbox_core::model::{Site, TvConfig};
    use vbox_core::sources::spider::list_categories;
    use vbox_core::spider::catvod::families::family_of;

    // family_of 是错误信息与观测的唯一入口，必须与 classify 一样先 trim。
    assert_eq!(family_of("csp_Kkys "), Some("kkys"));
    assert_eq!(family_of("\tcsp_SP33\n"), Some("sp360"));

    // 分发链同样不能把带空白的 api 交给 js/py 启发式；SixV 分类是静态表，不触网。
    let site: Site = serde_json::from_value(serde_json::json!({
        "key": "sixv-space",
        "name": "SixV space",
        "type": 3,
        "api": " csp_SixV "
    }))
    .expect("构造 Site");
    let cats = list_categories(&TvConfig::default(), &site).expect("空白 api 应仍能命中原生家族");
    assert!(!cats.is_empty());
}
