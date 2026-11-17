//! 判据 53：jar 资产画像 ↔ 本地 dex 分析产物 provenance 对账。
//!
//! `profiles.json` 里 20 个 jar 的 `sha256 / bytes / classes / spiders` 是本地
//! 分析后写死的汇总值，用来判断「这个 api 的 jar 是否已被逆向覆盖」。如果换了
//! jar 却只改 sha256、或手调画像数字，站点画像会继续宣称「原生就绪」，而实际
//! jar 内容早已漂移。这里按 sha256 把画像逐 jar 对回
//! `_rea/samples/library/dexinfo.json` 的**分析观测**：
//!   * `bytes` 与 dexinfo 的文件字节数一致；
//!   * `classes == Σ dexes[].classCount`；
//!   * `spiders == Σ dexes[].spiderClasses.length`（spiderClasses 是对象数组）；
//!   * 反向：所有分析出 spider 类的 jar 都必须登记在 profiles（无遗漏）。
//!
//! `_rea/` 不入库，CI 上缺失时与其它 `_rea` 判据同口径软跳过；本地则必须通过。
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crates/vbox_core -> crates -> 仓库根
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates 目录")
        .parent()
        .expect("仓库根")
        .to_path_buf()
}

fn load_json(path: &PathBuf) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{} 不是合法 JSON: {e}", path.display())))
}

fn profiles_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/profiles.json")
}

fn dexinfo_path() -> PathBuf {
    repo_root().join("_rea/samples/library/dexinfo.json")
}

/// 单个 jar 的分析观测：字节数 / 类总数 / spider 类数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DexMetrics {
    bytes: u64,
    classes: u64,
    spiders: u64,
}

fn dex_metrics(jar: &Value) -> DexMetrics {
    let dexes = jar["dexes"].as_array().unwrap_or_else(|| panic!("dexinfo jar 缺 dexes 数组: {jar}"));
    let mut classes = 0u64;
    let mut spiders = 0u64;
    for dex in dexes {
        classes += dex["classCount"].as_u64().unwrap_or(0);
        let list = dex["spiderClasses"]
            .as_array()
            .unwrap_or_else(|| panic!("dexinfo 的 spiderClasses 不是数组: {dex}"));
        spiders += list.len() as u64;
    }
    DexMetrics {
        bytes: jar["bytes"].as_u64().unwrap_or_else(|| panic!("dexinfo jar 缺 bytes: {jar}")),
        classes,
        spiders,
    }
}

fn dex_by_sha(dexinfo: &Value) -> BTreeMap<String, DexMetrics> {
    let jars = dexinfo["jars"].as_array().expect("dexinfo 缺 jars 数组");
    let mut map = BTreeMap::new();
    for jar in jars {
        let sha = jar["sha256"].as_str().unwrap_or_else(|| panic!("dexinfo jar 缺 sha256: {jar}"));
        map.insert(sha.to_string(), dex_metrics(jar));
    }
    map
}

#[test]
fn jar_metrics_match_dex_analysis() {
    let Some(dexinfo) = load_json(&dexinfo_path()) else {
        eprintln!("skip: {} 不存在（CI 无 _rea 快照）", dexinfo_path().display());
        return;
    };
    let profiles = load_json(&profiles_path()).expect("profiles.json 必须存在");
    let dex = dex_by_sha(&dexinfo);
    let jars = profiles["jars"].as_array().expect("profiles 缺 jars 数组");
    assert!(!jars.is_empty(), "profiles.jars 为空");

    let mut errors = Vec::new();
    for jar in jars {
        let name = jar["name"].as_str().unwrap_or("<未命名>");
        let sha = jar["sha256"].as_str().unwrap_or_else(|| panic!("画像 jar 缺 sha256: {name}"));
        let Some(observed) = dex.get(sha) else {
            errors.push(format!("{name}: sha256 {sha} 在 dexinfo.json 中找不到分析记录"));
            continue;
        };
        let want = (
            jar["bytes"].as_u64().unwrap_or(0),
            jar["classes"].as_u64().unwrap_or(0),
            jar["spiders"].as_u64().unwrap_or(0),
        );
        let got = (observed.bytes, observed.classes, observed.spiders);
        if want != got {
            errors.push(format!(
                "{name}: 画像 (bytes,classes,spiders)={want:?} != 分析产物 {got:?}（sha256 {sha}）"
            ));
        }
    }
    assert!(errors.is_empty(), "jar 画像与分析产物脱节（{} 处）:\n{}", errors.len(), errors.join("\n"));
}

#[test]
fn every_analyzed_spider_jar_is_registered() {
    let Some(dexinfo) = load_json(&dexinfo_path()) else {
        eprintln!("skip: {} 不存在（CI 无 _rea 快照）", dexinfo_path().display());
        return;
    };
    let profiles = load_json(&profiles_path()).expect("profiles.json 必须存在");
    let dex = dex_by_sha(&dexinfo);
    let registered: BTreeSet<String> = profiles["jars"]
        .as_array()
        .expect("profiles 缺 jars 数组")
        .iter()
        .map(|j| j["sha256"].as_str().expect("画像 jar 缺 sha256").to_string())
        .collect();

    let missing: Vec<String> = dex
        .iter()
        .filter(|(sha, m)| m.spiders > 0 && !registered.contains(*sha))
        .map(|(sha, m)| format!("{sha}（{} 类 / {} spider）", m.classes, m.spiders))
        .collect();
    assert!(
        missing.is_empty(),
        "有 spider 类的分析 jar 未登记进 profiles（{} 个）:\n{}",
        missing.len(),
        missing.join("\n")
    );
}
