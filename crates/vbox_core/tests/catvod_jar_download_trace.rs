//! 判据 65：下载清单（`index.json`）→ 去重 → 分析清单（`dexinfo.json`）→ 画像的 provenance 闭环。
//!
//! 判据 53/54/59/60/63 都从 `dexinfo.json` 出发，证明的是「分析产物 ↔ jar 字节 ↔ 画像」。
//! 但**下载阶段**到分析阶段之间还有一跳从未被校验：
//! `_rea/samples/library/index.json` 记录 28 次下载（21 个唯一 jar + 7 条 `dup` 重复记录），
//! 如果重命名/去重脚本改坏、或有人手填了 hash，dexinfo 与画像仍然自洽，下载来源链却断了。
//!
//! 本判据断言：
//! - 21 条 canonical 记录与 21 条 dexinfo 记录一一对应（file basename、bytes、md5、sha256 全等）；
//! - 7 条 `dup` 记录只保留 `file`/`from`/`dup`，不带 hash，且指向的文件真实存在；
//! - 28 条 `from` 下载路径互不重复（同一次下载不会被记两遍）；
//! - 画像的 20 个 jar 全部来自 canonical 集合，唯一落选的 jar 不含任何 spider 类。
//!
//! `_rea/` 缺失时 CI 软跳过。

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;
use vbox_core::spider::catvod::registry;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("仓库根目录")
        .to_path_buf()
}

fn base_name(path: &str) -> String {
    path.replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

fn load(name: &str) -> Option<(Value, Value, Value)> {
    let dir = root().join("_rea/samples/library");
    let index = std::fs::read_to_string(dir.join("index.json")).ok()?;
    let dex = std::fs::read_to_string(dir.join("dexinfo.json")).ok()?;
    Some((
        serde_json::from_str(&index).expect("index.json 不是合法 JSON"),
        serde_json::from_str(&dex).expect("dexinfo.json 不是合法 JSON"),
        Value::String(name.to_string()),
    ))
}

fn spider_class_total(dex: &Value) -> u64 {
    dex["dexes"]
        .as_array()
        .map(|dexes| {
            dexes
                .iter()
                .map(|d| d["spiderClasses"].as_array().map(|a| a.len() as u64).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

#[test]
fn download_index_matches_analysis_inventory() {
    let Some((index, dexinfo, _)) = load("index.json") else {
        eprintln!("跳过判据 65：_rea/samples/library 缺失（CI 不提交逆向产物）");
        return;
    };
    let records = index["jars"].as_array().expect("index.json 缺少 jars");
    let canonical: Vec<&Value> = records.iter().filter(|r| r["dup"] != Value::Bool(true)).collect();
    let dups: Vec<&Value> = records.iter().filter(|r| r["dup"] == Value::Bool(true)).collect();
    assert_eq!(canonical.len(), 21, "canonical 下载记录应为 21 条");
    assert_eq!(dups.len(), 7, "dup 记录应为 7 条（28 次下载 → 21 个唯一 jar）");
    assert_eq!(records.len(), 28, "下载记录总数应为 28");

    let dex_records = dexinfo["jars"].as_array().expect("dexinfo.json 缺少 jars");
    assert_eq!(dex_records.len(), 21, "dexinfo 分析记录应为 21 条");
    let dex_by_name: std::collections::BTreeMap<String, &Value> = dex_records
        .iter()
        .map(|d| (base_name(d["file"].as_str().unwrap_or_default()), d))
        .collect();
    let canonical_names: BTreeSet<String> =
        canonical.iter().map(|c| base_name(c["file"].as_str().unwrap_or_default())).collect();
    let dex_names: BTreeSet<String> = dex_by_name.keys().cloned().collect();
    assert_eq!(canonical_names, dex_names, "canonical 记录与 dexinfo 记录的文件集合不一致");

    let mut errors = Vec::new();
    for record in &canonical {
        let name = base_name(record["file"].as_str().unwrap_or_default());
        let Some(dex) = dex_by_name.get(&name) else {
            errors.push(format!("{name}: dexinfo 缺少同名记录"));
            continue;
        };
        for field in ["bytes", "md5", "sha256"] {
            if record[field] != dex[field] {
                errors.push(format!(
                    "{name}: index.{field}={:?} vs dexinfo={:?}",
                    record[field], dex[field]
                ));
            }
        }
        if !record["isZip"].is_boolean() {
            errors.push(format!("{name}: index 缺少 isZip 布尔字段"));
        }
    }
    assert!(errors.is_empty(), "下载清单与分析清单不一致：\n{}", errors.join("\n"));

    let froms: BTreeSet<String> = records
        .iter()
        .map(|r| r["from"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(froms.len(), 28, "28 条下载记录应来自 28 个互不重复的 from 路径");
}

#[test]
fn duplicate_records_never_masquerade_as_analyzed() {
    let Some((index, dexinfo, _)) = load("index.json") else {
        eprintln!("跳过判据 65：_rea/samples/library 缺失");
        return;
    };
    let records = index["jars"].as_array().expect("index.json 缺少 jars");
    let jars_dir = root().join("_rea/samples/library/jars");
    let canonical: Vec<&Value> = records.iter().filter(|r| r["dup"] != Value::Bool(true)).collect();
    let dups: Vec<&Value> = records.iter().filter(|r| r["dup"] == Value::Bool(true)).collect();

    let mut errors = Vec::new();
    for dup in &dups {
        let name = base_name(dup["file"].as_str().unwrap_or_default());
        if !jars_dir.join(&name).exists() {
            errors.push(format!("dup 记录 {name}: 指向的文件不存在"));
        }
        for field in ["bytes", "md5", "sha256", "isZip"] {
            if dup.get(field).is_some() {
                errors.push(format!("dup 记录 {name}: 不应携带 {field} 字段"));
            }
        }
        if dup["from"].as_str().unwrap_or_default().is_empty() {
            errors.push(format!("dup 记录 {name}: from 为空"));
        }
        let target = canonical.iter().find(|c| base_name(c["file"].as_str().unwrap_or_default()) == name);
        if target.is_none() {
            errors.push(format!("dup 记录 {name}: 没有对应的 canonical 记录"));
        }
    }
    assert!(errors.is_empty(), "dup 记录不成对：\n{}", errors.join("\n"));

    let canonical_shas: BTreeSet<String> = canonical
        .iter()
        .map(|c| c["sha256"].as_str().unwrap_or_default().to_string())
        .collect();
    let profiles = registry::profiles();
    assert_eq!(profiles.jars.len(), 20, "画像 jar 数应为 20");
    for jar in &profiles.jars {
        assert!(
            canonical_shas.contains(&jar.sha256),
            "画像 jar {}（sha256 {}）不在下载 canonical 集合中",
            jar.name,
            jar.sha256
        );
    }
    let dex_records = dexinfo["jars"].as_array().expect("dexinfo.json 缺少 jars");
    let extra: Vec<&Value> = canonical
        .iter()
        .copied()
        .filter(|c| !profiles.jars.iter().any(|j| j.sha256 == c["sha256"].as_str().unwrap_or_default()))
        .collect();
    assert_eq!(extra.len(), 1, "应恰好有 1 个 canonical jar 未进画像（非 spider jar）");
    for record in extra {
        let name = base_name(record["file"].as_str().unwrap_or_default());
        let dex = dex_records
            .iter()
            .find(|d| base_name(d["file"].as_str().unwrap_or_default()) == name)
            .unwrap_or_else(|| panic!("dexinfo 缺少 {name}"));
        assert_eq!(spider_class_total(dex), 0, "落选 jar {name} 不应包含 spider 类");
    }
    let non_zip = canonical.iter().filter(|c| c["isZip"] == Value::Bool(false)).count();
    assert_eq!(non_zip, 1, "canonical 中应恰好 1 个非 ZIP 载体（.jpg 裸 dex）");
}
