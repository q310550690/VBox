//! 判据 46：每个原生家族都必须能回溯到「api → jar → REA 证据附录 → 回归测试」四件套（第 163 轮）。
//!
//! 前几条判据分别锁住了：画像↔运行时互锁（39）、分类结论↔画像（40）、站点分派（43）、jar 台账闭包（45）。
//! 但它们都没有回答一个问题：**这个家族的实现依据写在文档的哪里？** 一旦 REA 文档被重构/截断/漏写，
//! 代码仍然能跑、测试仍然能绿，只是「为什么这样做」的证据消失了。
//!
//! 本判据用内嵌画像 + 家族分派做全量互锁：
//! - 每个家族的每个 api 必须是 rulepack、必须至少归属一个 jar（jar 在台账中）；
//! - 每个家族必须有专属回归测试文件 `tests/catvod_<family>_family.rs`；
//! - 每个 api 的字符串必须出现在 REA 文档中（作为「有据可查」的最低要求）；
//! - 旧规则引擎（XBPQ/XYQHiker 系列）不是 `families/` 家族，必须与家族集合互斥，且文档中有引擎级说明。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use vbox_core::spider::catvod::{families, registry};

const LEGACY_RULE_ENGINES: [&str; 5] = [
    "csp_XBPQ",
    "csp_XBPQ_upgraded",
    "csp_XYQHiker",
    "csp_XYQHikerAL",
    "csp_XYQHikerV2",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("定位仓库根目录失败")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", p.display()))
}

/// 家族 → 该家族接管的 api 集合（来自运行时 `families::family_of`）。
fn family_apis() -> BTreeMap<String, BTreeSet<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for api in registry::profiles().apis.keys() {
        if let Some(family) = families::family_of(api) {
            map.entry(family.to_string()).or_default().insert(api.clone());
        }
    }
    map
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn every_family_api_is_traceable_to_jar_test_and_rea() {
    let p = registry::profiles();
    let rea = read("docs/type3兼容/type3-路线-REA原生重写.md");
    let jar_names: BTreeSet<String> = p.jars.iter().map(|j| j.name.clone()).collect();
    let map = family_apis();

    assert_eq!(map.len(), 77, "运行时家族数应恰为 76，实际 {}", map.len());
    let mut apis_checked = 0usize;
    for (family, apis) in &map {
        assert!(!apis.is_empty(), "家族 {family} 没有接管任何 api");

        let test = repo_root().join(format!("crates/vbox_core/tests/catvod_{family}_family.rs"));
        assert!(test.exists(), "家族 {family} 缺少专属回归测试 {}", test.display());

        for api in apis {
            apis_checked += 1;
            let profile = p.apis.get(api).expect("api 必在画像中");
            assert_eq!(
                profile.strategy().as_str(),
                "rulepack",
                "家族 {family} 的 api {api} 画像不是 rulepack"
            );
            assert!(
                !profile.jars.is_empty(),
                "家族 {family} 的 api {api} 没有来源 jar"
            );
            for jar in &profile.jars {
                assert!(jar_names.contains(jar), "api {api} 引用了未知 jar {jar}");
            }
            assert!(
                rea.contains(api.as_str()),
                "api {api}（家族 {family}）在 REA 证据文档中没有任何记载"
            );
        }
    }
    assert!(
        apis_checked >= 88,
        "家族覆盖 api 数异常: {apis_checked}（应 ≥88）"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn legacy_rule_engines_are_disjoint_from_families_and_documented() {
    let p = registry::profiles();
    let rea = read("docs/type3兼容/type3-路线-REA原生重写.md");

    // 5 个名字里只有 3 个是画像中的真实 api（XBPQ_upgraded / XYQHikerV2 是分派别名），
    // 但无论是否出现在画像里，都不能被 families 家族接管。
    let mut present_legacy = 0usize;
    for api in LEGACY_RULE_ENGINES {
        assert!(
            families::family_of(api).is_none(),
            "旧规则引擎 {api} 不应被任何 families 家族接管"
        );
        if let Some(profile) = p.apis.get(api) {
            present_legacy += 1;
            assert_eq!(
                profile.strategy().as_str(),
                "rulepack",
                "旧规则引擎 {api} 应保持 rulepack"
            );
        }
    }
    assert_eq!(present_legacy, 3, "画像中的旧规则引擎应为 3 个（XBPQ/XYQHiker/XYQHikerAL）");

    // 引擎级文档：REA 至少要有 XBPQ / XYQHiker 两族说明（不是「每个 api 一节」）。
    assert!(rea.contains("XBPQ"), "REA 文档缺少 XBPQ 规则引擎说明");
    assert!(rea.contains("XYQHiker"), "REA 文档缺少 XYQHiker 规则引擎说明");

    // 数量互锁：rulepack = 家族接管的 api + 旧规则引擎 + （无）其它。
    let family_api_count: usize = family_apis().values().map(|s| s.len()).sum();
    let rulepack: usize = p
        .apis
        .values()
        .filter(|a| a.strategy().as_str() == "rulepack")
        .count();
    assert_eq!(
        rulepack,
        family_api_count + present_legacy,
        "rulepack 数 ≠ 家族 api 数 + 画像内旧规则引擎数"
    );
}
