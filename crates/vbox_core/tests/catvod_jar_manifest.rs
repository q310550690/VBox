//! 判据 45：jar 资产清单的自洽性与「引用闭包」（第 162 轮）。
//!
//! 背景：`profiles.json` 的 `jars` 是「我们到底分析了哪些 jar」的唯一台账，
//! 每个 api 的 `jars` 字段则是「这个 api 从哪些 jar 里来」的反查索引。
//! 二者若漂移，会出现两类沉默错误：
//! - api 引用了清单里不存在的 jar（来源不可追溯，REA 证据链断开）；
//! - 清单里有从未被任何 api 引用的孤儿 jar（不知道它为什么在台账里）。
//!
//! 本判据不联网、不读 `_rea/`，只用内嵌画像做闭包校验，因此在 CI 上也能跑；
//! 另用 guarded 语义锁住「守卫壳 jar（只有几十个类）→ 只被 unsupported 的 *Guard api 引用」这条已知事实，
//! 防止将来把壳 jar 误接进 rulepack 家族。

use std::collections::{BTreeMap, BTreeSet};

use vbox_core::spider::catvod::registry;

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn jar_names() -> BTreeSet<String> {
    registry::profiles().jars.iter().map(|j| j.name.clone()).collect()
}

#[test]
fn jar_manifest_entries_are_well_formed() {
    let p = registry::profiles();
    assert_eq!(p.jars.len(), 20, "jar 台账应恰有 20 个条目");
    assert_eq!(p.totals.jars as usize, p.jars.len(), "totals.jars 与 jars 长度不一致");

    let mut seen = BTreeSet::new();
    for j in &p.jars {
        assert!(!j.name.is_empty(), "jar 名称为空");
        assert!(seen.insert(j.name.clone()), "jar 名称重复: {}", j.name);
        // 采集器按「下载文件名」命名：可能是 .jar / 伪装 .jpg/.png，
        // 也可能是无扩展名的 cloaking URL（如 8ad69276-gd5ic…），后者只有无点号时才允许。
        let known_ext = [".jar", ".jpg", ".png"].iter().any(|e| j.name.ends_with(e));
        let extensionless = !j.name.contains('.');
        assert!(
            known_ext || extensionless,
            "jar 名称扩展非法（应为 .jar/.jpg/.png 或无扩展名 cloaking）: {}",
            j.name
        );
        assert!(hex64(&j.sha256), "jar sha256 非法: {} => {}", j.name, j.sha256);
        assert!(j.bytes > 0, "jar 字节数必须 >0: {}", j.name);
        assert!(j.classes > 0, "jar 类数必须 >0: {}", j.name);
        assert!(j.spiders > 0, "jar spider 数必须 >0: {}", j.name);
        assert!(
            j.spiders <= j.classes,
            "jar {} 的 spider 数({}) 不应超过类数({})",
            j.name,
            j.spiders,
            j.classes
        );
    }
}

#[test]
fn jar_references_are_a_closed_set() {
    let p = registry::profiles();
    let names = jar_names();
    let mut refs: BTreeMap<String, u32> = p.jars.iter().map(|j| (j.name.clone(), 0)).collect();

    for (api, profile) in &p.apis {
        for jar in &profile.jars {
            assert!(
                names.contains(jar),
                "api {api} 引用了台账中不存在的 jar: {jar}"
            );
            *refs.get_mut(jar).expect("jar 必在 refs 中") += 1;
        }
    }

    for (jar, count) in &refs {
        assert!(*count > 0, "孤儿 jar（未被任何 api 引用）: {jar}");
    }
    let referenced: BTreeSet<String> = refs.keys().cloned().collect();
    assert_eq!(
        referenced, names,
        "api 引用的 jar 集合与台账不闭合：台账多 {:?}、引用多 {:?}",
        names.difference(&referenced).collect::<Vec<_>>(),
        referenced.difference(&names).collect::<Vec<_>>()
    );
}

#[test]
fn every_rulepack_api_owns_at_least_one_jar() {
    let p = registry::profiles();
    let names = jar_names();
    let rulepack: Vec<(&String, &registry::ApiProfile)> = p
        .apis
        .iter()
        .filter(|(_, a)| a.strategy().as_str() == "rulepack")
        .collect();
    assert!(
        rulepack.len() >= 90,
        "rulepack api 数量异常: {}（应 ≥90）",
        rulepack.len()
    );
    for (api, profile) in &rulepack {
        assert!(
            !profile.jars.is_empty(),
            "rulepack api {api} 没有来源 jar（原生实现必须可追溯到 jar）"
        );
        for jar in &profile.jars {
            assert!(names.contains(jar), "rulepack api {api} 引用了未知 jar: {jar}");
        }
    }
}

#[test]
fn guarded_jars_are_only_used_by_guard_shells() {
    let p = registry::profiles();
    let guarded_jars: BTreeSet<String> = p
        .jars
        .iter()
        .filter(|j| j.guarded)
        .map(|j| (j.name.clone(), j.spiders))
        .filter(|(_, spiders)| *spiders < 100)
        .map(|(name, _)| name)
        .collect();
    assert!(
        (3..10).contains(&guarded_jars.len()),
        "守卫壳 jar 数量异常: {}（预期 3–9 个）",
        guarded_jars.len()
    );

    let mut guarded_api_count = 0usize;
    for (api, profile) in &p.apis {
        if !profile.guarded {
            continue;
        }
        guarded_api_count += 1;
        assert_eq!(
            profile.strategy().as_str(),
            "unsupported",
            "守卫壳 api {api} 不应该是 {:?}（壳只能标记为 unsupported）",
            profile.strategy().as_str()
        );
        for jar in &profile.jars {
            assert!(
                guarded_jars.contains(jar),
                "守卫壳 api {api} 引用了非壳 jar: {jar}"
            );
        }
    }
    assert!(
        guarded_api_count >= 30,
        "守卫壳 api 数量异常: {guarded_api_count}（应 ≥30）"
    );

    // 反向：非壳 jar 的类数远大于壳 jar，避免把普通 jar 误标 guarded。
    for j in &p.jars {
        if j.guarded {
            assert!(j.classes < 200, "壳 jar {} 的类数过多: {}", j.name, j.classes);
        } else {
            assert!(
                j.classes >= 200,
                "非壳 jar {} 的类数过少（疑似漏标 guarded）: {}",
                j.name,
                j.classes
            );
        }
    }
}
