//! 判据 72：订阅里的 jar 引用 ↔ 本地 jar 台账。
//!
//! 站点配置引用 26 个 jar、14 个不同 URL；其中 13 行带 `jarMd5`，去重后 9 个 md5。
//! 这些是上游订阅的**声明**，此前没有任何判据把它们与本地下载台账对上：
//! 某个 md5 其实已经下载（或本地样本丢失），画像与覆盖报告都仍显示「20 个 jar」。
//!
//! 审计发现三类上游数据事实，本判据把它们冻结为可复核账本：
//! 1. 同一动态端点 `http://tv.nxog.top/m/jar.php?id=ou` 在三个订阅里被钉了 3 个不同 md5；
//! 2. 同名 `1.jar` 实际是 3 个不同 jar（无 md5 / meisha bb155c3f / 另一个 6120d08f）；
//! 3. 9 个声明 md5 中只有 meisha 的 `bb155c3f0133bbce4756ad52003f5968` 在本地样本内，
//!    其余 8 个来自不可达站点，尚未下载。
//!
//! 断言：
//! - 26 条引用 / 14 个 URL / 13 条带 md5 / 9 个不同 md5；
//! - 多 md5 的 URL 必须恰好只有 nxog 动态端点（3 个值），其他 URL 最多 1 个；
//! - 每个 jarMd5 必须是 32 位小写 hex；
//! - 声明 md5 与本地 canonical 台账的交集必须恰好是 meisha md5；其余 8 个缺席。
//!
//! `_rea/` 缺失时 CI 软跳过。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

const MEISHA_MD5: &str = "bb155c3f0133bbce4756ad52003f5968";
const DYNAMIC_URL: &str = "http://tv.nxog.top/m/jar.php?id=ou";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("仓库根目录")
        .to_path_buf()
}

fn load(name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(root().join(name)).ok()?;
    Some(serde_json::from_str(&text).expect("JSON 必须合法"))
}

fn is_md5(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn jar_references_are_well_formed_and_dynamic_endpoint_is_isolated() {
    let Some(sites) = load("_rea/samples/inventory/sites.json") else {
        return;
    };
    let rows: Vec<&Value> = sites["sites"]
        .as_array()
        .expect("sites 必须是数组")
        .iter()
        .filter(|row| row["jar"].is_string())
        .collect();
    assert_eq!(rows.len(), 26, "带 jar 的站点应为 26 行");

    let mut urls: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut md5s = BTreeSet::new();
    let mut with_md5 = 0;
    for row in &rows {
        let url = row["jar"].as_str().unwrap();
        assert!(
            url.starts_with("http://") || url.starts_with("https://"),
            "jar 必须是绝对 URL：{url}"
        );
        let entry = urls.entry(url.to_string()).or_default();
        let declared = row["jarMd5"].as_str();
        if let Some(md5) = declared {
            with_md5 += 1;
            assert!(is_md5(md5), "jarMd5 必须是 32 位小写 hex：{md5}（{url}）");
            md5s.insert(md5.to_string());
            entry.insert(md5.to_string());
        }
    }
    assert_eq!(urls.len(), 14, "不同 jar URL 应为 14 个");
    assert_eq!(with_md5, 13, "带 jarMd5 的引用应为 13 条");
    assert_eq!(md5s.len(), 9, "去重后应有 9 个不同 jarMd5");

    let multi: BTreeMap<String, BTreeSet<String>> = urls
        .into_iter()
        .filter(|(_, values)| values.len() > 1)
        .collect();
    assert_eq!(multi.len(), 1, "只允许一个动态端点声明多个 md5：{multi:?}");
    let dynamic = multi
        .get(DYNAMIC_URL)
        .expect("动态端点必须是 nxog jar.php?id=ou");
    assert_eq!(dynamic.len(), 3, "动态端点应被钉了 3 个不同 md5：{dynamic:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn declared_md5s_reconcile_with_local_canonical_ledger() {
    let Some(sites) = load("_rea/samples/inventory/sites.json") else {
        return;
    };
    let Some(index) = load("_rea/samples/library/index.json") else {
        return;
    };

    let declared: BTreeSet<String> = sites["sites"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["jarMd5"].as_str())
        .map(|md5| md5.to_ascii_lowercase())
        .collect();
    assert_eq!(declared.len(), 9);

    let canonical: BTreeSet<String> = index["jars"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["dup"].as_bool() != Some(true))
        .filter_map(|record| record["md5"].as_str())
        .map(|md5| md5.to_ascii_lowercase())
        .collect();
    assert_eq!(canonical.len(), 21, "canonical 记录应有 21 个 jar");

    let present: BTreeSet<&String> = declared.intersection(&canonical).collect();
    assert_eq!(
        present,
        [MEISHA_MD5.to_string()].iter().collect::<BTreeSet<_>>(),
        "声明 md5 与本地台账的交集必须恰为 meisha"
    );

    let absent = declared.len() - present.len();
    assert_eq!(absent, 8, "其余 8 个声明 md5 尚未下载（不可达站点）");
}
