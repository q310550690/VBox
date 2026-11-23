//! 判据 60：每个带 `class` 的 api 画像指标（jars/methods/guarded）必须能由
//! dexinfo 逐项重算；受保护壳 api 不得被计入原生 rulepack。
//!
//! 背景：`gen-profiles.mjs` 从 dexinfo 的 `spiderClasses` 推导三件事——
//! 该类出现在哪些 jar、methods 取同一类在多个 dex 中的最大值、以及 guarded
//! 是否所有命中 jar 都受保护。这三项此前只由「生成器跑过一次」保证：若画像
//! 在 dexinfo 更新后没有重生成，站点数/策略/难度仍然自洽，但 api→jar 的归因
//! 已经过期。判据 55 只验证 class 存在与 super 可解析，methods 因口径疑问被跳过；
//! 本轮确认「profiles.methods == max(命中 dex 的 methods)」93/93 成立，故把它
//! 连同 jars/guarded 一起锁死。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/vbox_core 应位于仓库两级目录下")
        .to_path_buf()
}

fn load(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} 不是合法 JSON: {e}", path.display())))
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

struct DexFacts {
    /// api class 简名 -> 命中记录（jar 名、methods、该 jar 是否受保护）
    hits: BTreeMap<String, Vec<(String, u64, bool)>>,
}

fn dex_facts(dexinfo: &serde_json::Value) -> DexFacts {
    let mut hits: BTreeMap<String, Vec<(String, u64, bool)>> = BTreeMap::new();
    for jar in dexinfo["jars"].as_array().expect("dexinfo.jars 应为数组") {
        let jar_name = basename(jar["file"].as_str().unwrap_or_default()).to_string();
        let Some(dexes) = jar["dexes"].as_array() else { continue };
        let Some(dex) = dexes.first() else { continue };
        let guarded = jar["entries"].as_array().map_or(false, |entries| {
            entries
                .iter()
                .filter_map(|e| e.as_str())
                .any(|e| e.to_ascii_lowercase().contains("guard"))
        }) || dex["spiderClasses"].as_array().map_or(false, |classes| {
            classes
                .iter()
                .any(|c| c["name"].as_str().map_or(false, |n| n.ends_with("Guard")))
        });
        for class in dex["spiderClasses"].as_array().expect("dex.spiderClasses 应为数组") {
            let Some(name) = class["name"].as_str() else { continue };
            let methods = class["methods"].as_u64().unwrap_or(0);
            hits.entry(name.to_string())
                .or_default()
                .push((jar_name.clone(), methods, guarded));
        }
    }
    DexFacts { hits }
}

const CLASS_PREFIX: &str = "com.github.catvod.spider.";

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn class_bearing_api_metrics_match_dex_analysis() {
    let root = repo_root();
    let profiles = load(&root.join("crates/vbox_core/src/spider/catvod/profiles.json"))
        .expect("profiles.json 必须存在");
    let Some(dexinfo) = load(&root.join("_rea/samples/library/dexinfo.json")) else {
        eprintln!("_rea 不在当前检出，跳过判据 60（CI 软跳过）");
        return;
    };
    let facts = dex_facts(&dexinfo);
    let apis = profiles["apis"].as_object().expect("profiles.apis 应为对象");

    let mut checked = 0usize;
    let mut sentinels = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (api, entry) in apis {
        let Some(class) = entry["class"].as_str() else { continue };
        checked += 1;
        let Some(simple) = class.strip_prefix(CLASS_PREFIX) else {
            failures.push(format!("{api}: class {class:?} 不以 {CLASS_PREFIX} 开头"));
            continue;
        };
        let Some(hits) = facts.hits.get(simple) else {
            // 合法哨兵：类不在已下载 jar 中时，画像必须恰好是「不可用」状态，
            // 而不是悄悄沿用旧指标。
            let has_jar = entry["jars"].as_array().map_or(false, |a| !a.is_empty());
            if has_jar
                || entry["methods"].as_u64() != Some(0)
                || entry["guarded"].as_bool() != Some(false)
                || entry["strategy"].as_str() != Some("unsupported")
                || entry["reason"].as_str() != Some("class_not_in_downloaded_jars")
            {
                failures.push(format!(
                    "{api}: class {simple} 不在 dexinfo 中，但画像不是 class_not_in_downloaded_jars 哨兵（jars={:?}, methods={:?}, guarded={:?}, strategy={:?}, reason={:?}）",
                    entry["jars"], entry["methods"], entry["guarded"], entry["strategy"], entry["reason"]
                ));
            }
            sentinels += 1;
            continue;
        };
        let declared_jars: BTreeSet<&str> = entry["jars"]
            .as_array()
            .map_or_else(BTreeSet::new, |a| a.iter().filter_map(|j| j.as_str()).collect());
        let derived_jars: BTreeSet<&str> = hits.iter().map(|(jar, _, _)| jar.as_str()).collect();
        if declared_jars != derived_jars {
            failures.push(format!(
                "{api}: jars 不一致，画像 {:?}，重算 {:?}",
                declared_jars, derived_jars
            ));
        }
        let declared_methods = entry["methods"].as_u64().expect("methods 应为整数");
        let derived_methods = hits.iter().map(|(_, m, _)| *m).max().unwrap_or(0);
        if declared_methods != derived_methods {
            failures.push(format!(
                "{api}: methods 不一致，画像 {declared_methods}，重算 {derived_methods}"
            ));
        }
        let declared_guarded = entry["guarded"].as_bool().expect("guarded 应为布尔");
        let derived_guarded = hits.iter().all(|(_, _, g)| *g);
        if declared_guarded != derived_guarded {
            failures.push(format!(
                "{api}: guarded 不一致，画像 {declared_guarded}，重算 {derived_guarded}"
            ));
        }
        if declared_methods == 0 {
            failures.push(format!("{api}: methods 为 0，但该类在 jar 中有命中"));
        }
        if declared_jars.is_empty() {
            failures.push(format!("{api}: jars 为空，但该类在 jar 中有命中"));
        }
    }
    assert!(checked >= 150, "带 class 的 api 只有 {checked} 个，疑似画像被截断");
    assert!(
        failures.is_empty(),
        "api 指标无法由 dexinfo 重算（{} 处）：\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!(
        "判据 60：{checked} 个带 class 的 api 指标全部可由 dexinfo 重算（{sentinels} 个为 class_not_in_downloaded_jars 哨兵）"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn guarded_shells_are_never_native() {
    let root = repo_root();
    let profiles = load(&root.join("crates/vbox_core/src/spider/catvod/profiles.json"))
        .expect("profiles.json 必须存在");
    let apis = profiles["apis"].as_object().expect("profiles.apis 应为对象");

    let mut guarded_apis = 0usize;
    let mut rulepack_without_class: Vec<&str> = Vec::new();
    let mut guarded_native: Vec<&str> = Vec::new();
    for (api, entry) in apis {
        let strategy = entry["strategy"].as_str().unwrap_or_default();
        if entry["guarded"].as_bool() == Some(true) {
            guarded_apis += 1;
            if strategy == "rulepack" {
                guarded_native.push(api);
            }
        }
        if strategy == "rulepack" && entry["class"].as_str().is_none() {
            rulepack_without_class.push(api);
        }
    }
    assert!(
        guarded_apis >= 1,
        "画像中没有任何受保护壳 api，guarded 语义可能被删除"
    );
    assert!(
        guarded_native.is_empty(),
        "受保护 jar 的壳 api 不得计入原生 rulepack：{guarded_native:?}"
    );
    assert!(
        rulepack_without_class.is_empty(),
        "rulepack api 必须带 class，缺失：{rulepack_without_class:?}"
    );
    println!("判据 60：{guarded_apis} 个受保护壳 api 均未计入原生；rulepack 全部带 class");
}
