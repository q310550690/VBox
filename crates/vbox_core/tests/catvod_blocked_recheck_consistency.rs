//! 不变量：画像中 `blockedReason = "site_dead"` 的 api 集合，
//! 必须与 `docs/审计报告/阻断复核.md` 的报告行**双向完全一致**。
//!
//! 背景：`site_dead` 是「上游存活快照」的结论；复核报告（第 76 轮）是它的可复查记录。
//! 若两者漂移，说明「画像被改过但没复核」或「复核过但画像没同步」，两者都会让
//! 「100% 兼容」的分母失真。本测试把该一致性固化进 cargo test。

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(rel)
}

fn read(rel: &str) -> String {
    let path = workspace_path(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

fn site_dead_apis() -> BTreeSet<String> {
    let raw = read("crates/vbox_core/src/spider/catvod/profiles.json");
    let profiles: serde_json::Value = serde_json::from_str(&raw).expect("profiles.json 不是合法 JSON");
    profiles["apis"]
        .as_object()
        .expect("profiles.apis 不是对象")
        .iter()
        .filter(|(_, v)| v.get("blockedReason").and_then(|r| r.as_str()) == Some("site_dead"))
        .map(|(api, _)| api.clone())
        .collect()
}

fn report_apis() -> BTreeSet<String> {
    let doc = read("docs/审计报告/阻断复核.md");
    doc.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("| `")?;
            let (api, _) = rest.split_once('`')?;
            if api.starts_with("csp_") { Some(api.to_string()) } else { None }
        })
        .collect()
}

#[test]
fn profile_site_dead_set_matches_recheck_report() {
    let dead = site_dead_apis();
    let rows = report_apis();

    let missing: Vec<_> = dead.difference(&rows).cloned().collect();
    let extra: Vec<_> = rows.difference(&dead).cloned().collect();

    assert!(
        missing.is_empty() && extra.is_empty(),
        "site_dead 画像与复核报告不一致：\n  报告缺少: {missing:?}\n  报告多出: {extra:?}\n（修复：node _rea/tools/blocked-recheck.mjs --write 后提交）"
    );
    assert!(dead.len() >= 3, "site_dead 数量异常偏少: {}", dead.len());
}

#[test]
fn recheck_summary_count_matches_rows() {
    let dead = site_dead_apis();
    let doc = read("docs/审计报告/阻断复核.md");
    let expected = format!("复核 api：**{}**", dead.len());
    assert!(
        doc.contains(&expected),
        "复核报告汇总行与画像数量不一致（期望 {expected:?}）；修复：node _rea/tools/blocked-recheck.mjs --write"
    );
}
