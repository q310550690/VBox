//! 家族分派接线回归（第 52 轮）。
//!
//! 五个 spider 入口（分类/列表/详情/搜索/播放）必须对每个原生家族都有分支；
//! 漏接一个入口意味着该家族在该能力上会掉进兜底错误，属于静默功能缺失。
//!
//! 约束（静态文本检查，无网络）：
//! 1. `families/mod.rs` 声明了每个家族模块；
//! 2. `sources/spider.rs` 有对应 `use … as catvod_<family>` 导入；
//! 3. `sources/spider.rs` 中 `catvod_<family>::handles` 恰好出现 **5** 次（五个入口）；
//! 4. `catvod_` 分支总数 = 家族数 × 5；出现的 `catvod_<x>` 标识符集合与家族集合一致（无残留/无漏接）。

use std::collections::BTreeSet;

use vbox_core::spider::catvod::families;
use vbox_core::spider::catvod::registry;

fn count(hay: &str, needle: &str) -> usize {
    hay.match_indices(needle).count()
}

#[test]
fn every_native_family_is_dispatched_from_all_five_entries() {
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let families_mod = std::fs::read_to_string(src_dir.join("spider/catvod/families/mod.rs"))
        .expect("families/mod.rs");
    let spider = std::fs::read_to_string(src_dir.join("sources/spider.rs")).expect("sources/spider.rs");

    // 家族集合从「已接线 api 的 family_of 结果」推导。
    let p = registry::profiles();
    let mut families_set = BTreeSet::new();
    for api in p.apis.keys() {
        if let Some(family) = families::family_of(api) {
            families_set.insert(family.to_string());
        }
    }
    assert!(
        families_set.len() >= 37,
        "原生家族数应 ≥ 37，实际 {}",
        families_set.len()
    );

    let mut errors = Vec::new();
    for family in &families_set {
        if !families_mod.contains(&format!("pub mod {family};")) {
            errors.push(format!("{family}: families/mod.rs 缺少 `pub mod {family};`"));
        }
        if !spider.contains(&format!("use crate::spider::catvod::families::{family} as catvod_{family};")) {
            errors.push(format!("{family}: sources/spider.rs 缺少 import 别名"));
        }
        let handles = count(&spider, &format!("catvod_{family}::handles"));
        if handles != 5 {
            errors.push(format!(
                "{family}: 分派分支应恰好 5 个（五入口），实际 {handles}"
            ));
        }
    }
    assert!(errors.is_empty(), "家族分派接线问题：\n{}", errors.join("\n"));

    // 总分支数 = 家族数 × 5；无多余/残留标识符。
    let total = count(&spider, "else if catvod_");
    assert_eq!(
        total,
        families_set.len() * 5,
        "else-if 分支总数应为家族数×5"
    );
    let mut idents = BTreeSet::new();
    let mut rest = spider.as_str();
    while let Some(pos) = rest.find("catvod_") {
        let tail = &rest[pos + "catvod_".len()..];
        let len = tail
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
            .count();
        if len > 0 {
            idents.insert(tail[..len].to_string());
        }
        rest = &tail[len..];
    }
    let unknown: Vec<_> = idents.difference(&families_set).cloned().collect();
    assert!(unknown.is_empty(), "spider.rs 存在未知家族标识符：{unknown:?}");
}
