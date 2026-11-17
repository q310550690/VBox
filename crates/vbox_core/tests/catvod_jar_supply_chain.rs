//! 判据 75：jar 供应链文档 ↔ 站点引用 ↔ 本地下载台账。
//!
//! 判据 72 冻结了 26 条引用 / 14 个 URL / 9 个声明 md5 的形状，但没有一份
//! 「哪些 jar 还没下载、由哪个站点引用、复活后怎么补」的可执行清单。
//! `docs/审计报告/jar供应链.md` 内嵌 ```json 机器可读清单，本判据解析它并与
//! `sites.json` / `library/index.json` 双向对账，防止文档与数据各自漂移。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

const DOC: &str = "docs/审计报告/jar供应链.md";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("仓库根目录")
        .to_path_buf()
}

fn read(rel: &str) -> Option<String> {
    std::fs::read_to_string(root().join(rel)).ok()
}

fn payload(doc: &str) -> Value {
    let start = doc
        .find("## 二、机器可读清单")
        .expect("文档必须有机器可读清单小节");
    let fence = doc[start..].find("```json").expect("清单必须是 json 代码块") + start;
    let body_start = doc[fence..].find('\n').expect("json 代码块必须有换行") + fence + 1;
    let body_end = doc[body_start..]
        .find("\n```")
        .expect("json 代码块必须闭合")
        + body_start;
    serde_json::from_str(&doc[body_start..body_end]).expect("内嵌清单必须是合法 JSON")
}

#[test]
fn supply_chain_payload_matches_inventory_and_ledger() {
    if !root().join("_rea").exists() {
        eprintln!("_rea 不存在，跳过历史证据判据");
        return;
    }
    let doc = read(DOC).expect("docs/审计报告/jar供应链.md 必须存在");
    let p = payload(&doc);

    // 站点侧：按 URL 聚合引用数、md5 集合。
    let sites_text = read("_rea/samples/inventory/sites.json").expect("sites.json");
    let sites: Value = serde_json::from_str(&sites_text).expect("sites.json 合法 JSON");
    let mut urls: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    let mut references = 0usize;
    for site in sites["sites"].as_array().expect("sites 数组") {
        let url = site["jar"].as_str().unwrap_or("").trim();
        if url.is_empty() {
            continue;
        }
        references += 1;
        let entry = urls.entry(url.to_string()).or_default();
        entry.0 += 1;
        let md5 = site["jarMd5"].as_str().unwrap_or("").trim().to_lowercase();
        if !md5.is_empty() {
            entry.1.insert(md5);
        }
    }
    let declared: BTreeSet<String> = urls.values().flat_map(|(_, m)| m.iter().cloned()).collect();

    // 台账侧：canonical md5 集合。
    let ledger_text = read("_rea/samples/library/index.json").expect("index.json");
    let ledger: Value = serde_json::from_str(&ledger_text).expect("index.json 合法 JSON");
    let canonical: Vec<&Value> = ledger["jars"]
        .as_array()
        .expect("jars 数组")
        .iter()
        .filter(|r| r.get("dup").is_none())
        .collect();
    let canonical_md5: BTreeSet<String> = canonical
        .iter()
        .map(|r| r["md5"].as_str().unwrap().to_lowercase())
        .collect();

    assert_eq!(p["totals"]["references"].as_u64(), Some(references as u64));
    assert_eq!(p["totals"]["urls"].as_u64(), Some(urls.len() as u64));
    assert_eq!(p["totals"]["md5s"].as_u64(), Some(declared.len() as u64));
    assert_eq!(p["totals"]["canonical"].as_u64(), Some(canonical.len() as u64));

    let payload_urls: BTreeMap<String, &Value> = p["urls"]
        .as_array()
        .expect("urls 数组")
        .iter()
        .map(|u| (u["url"].as_str().unwrap().to_string(), u))
        .collect();
    assert_eq!(
        payload_urls.keys().cloned().collect::<BTreeSet<_>>(),
        urls.keys().cloned().collect::<BTreeSet<_>>(),
        "文档 URL 集合必须与 sites.json 完全一致"
    );
    for (url, (refs, md5s)) in &urls {
        let row = payload_urls.get(url).expect("URL 必须存在");
        assert_eq!(row["refs"].as_u64(), Some(*refs as u64), "{url} 引用数");
        let doc_md5: BTreeSet<String> = row["md5s"]
            .as_array()
            .expect("md5s 数组")
            .iter()
            .map(|m| m.as_str().unwrap().to_lowercase())
            .collect();
        assert_eq!(&doc_md5, md5s, "{url} 声明 md5 集合");
        let expected_status = if md5s.is_empty() {
            "no-md5"
        } else if md5s.iter().all(|m| canonical_md5.contains(m)) {
            "present"
        } else {
            "absent"
        };
        assert_eq!(row["status"].as_str(), Some(expected_status), "{url} 状态");
    }

    // 声明 ∩ 台账 = 已下载；差集 = 缺席。
    let present: BTreeSet<String> = declared.intersection(&canonical_md5).cloned().collect();
    let absent: BTreeSet<String> = declared.difference(&canonical_md5).cloned().collect();
    let doc_present: BTreeSet<String> = p["present"]
        .as_array()
        .expect("present 数组")
        .iter()
        .map(|m| m.as_str().unwrap().to_lowercase())
        .collect();
    let doc_absent: BTreeSet<String> = p["absent"]
        .as_array()
        .expect("absent 数组")
        .iter()
        .map(|m| m.as_str().unwrap().to_lowercase())
        .collect();
    assert_eq!(doc_present, present, "已下载集合");
    assert_eq!(doc_absent, absent, "缺席集合");
    assert_eq!(doc_present.len(), 1, "当前应恰有 1 个声明 md5 已下载（meisha）");
    assert_eq!(doc_absent.len(), 8, "当前应恰有 8 个声明 md5 缺席");
}

#[test]
fn supply_chain_documents_absent_recovery() {
    if !root().join("_rea").exists() {
        eprintln!("_rea 不存在，跳过历史证据判据");
        return;
    }
    let doc = read(DOC).expect("docs/审计报告/jar供应链.md 必须存在");
    let p = payload(&doc);

    // 人类可读的逐 URL 表必须与机器清单条数一致。
    let url_rows = doc
        .lines()
        .filter(|l| l.trim_start().starts_with("| `http"))
        .count();
    assert_eq!(url_rows, p["urls"].as_array().unwrap().len(), "URL 明细行数");

    // 缺席清单：每个 md5 一行，且写明「复活后重下」动作。
    let section = doc
        .split("## 四、缺席清单")
        .nth(1)
        .and_then(|s| s.split("## 五、").next())
        .expect("必须有缺席清单小节");
    for md5 in p["absent"].as_array().unwrap() {
        let md5 = md5.as_str().unwrap();
        let row = section
            .lines()
            .find(|l| l.contains(md5))
            .unwrap_or_else(|| panic!("缺席清单必须列出 {md5}"));
        assert!(
            row.contains("复活后重下"),
            "{md5} 行必须写明复活后动作：{row}"
        );
    }

    // 复现命令与口径说明必须存在，避免文档退化成手工表格。
    assert!(doc.contains("node _rea/samples/inventory/jar-supply.mjs"));
    assert!(doc.contains("## 五、复现"));
    assert!(doc.contains("## 六、口径说明"));
    assert!(doc.contains("不表示该 URL 当前可访问"));
}
