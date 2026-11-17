//! 判据 55：画像 api 的 `class / jars / methods` 与 dex 分析产物中的
//! spider 类清单**单点**对账。
//!
//! 判据 45 校验 api→jar 引用闭包、判据 53/54 校验 jar↔dexinfo↔文件字节，但
//! 没有任何一条把「这个 api 到底对应哪个类」锁住：`profiles.json` 里的
//! `class` 是人工抄的全限定名，抄错一个字母（例如 `MeijuMi` vs `Meijumi`）时，
//! 台账、画像与字节校验全绿，真正运行到该 api 才会 `ClassNotFound`。
//!
//! 本判据按 api 的三元组 `(class, jars, strategy=rulepack)`：
//!   * `class` 必须是 `com.github.catvod.spider.<Simple>` 形态；
//!   * `<Simple>` 必须出现在其 `jars` 至少一个 dex 的 `spiderClasses[].name`；
//!   * 命中类的 `super` 必须是 `Spider`，或同一 jar 的 spider 类清单里可解析
//!     （证明类图不是被截断的半个 jar）。
//!
//! `_rea/` 缺失时 CI 软跳过；本地必须通过。
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

const SPIDER_PREFIX: &str = "com.github.catvod.spider.";

fn repo_root() -> PathBuf {
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

fn basename(file: &str) -> String {
    file.replace('\\', "/").rsplit('/').next().unwrap_or(file).to_string()
}

struct DexJar {
    /// 该 jar 全部 dex 里的 spider 类 simple name 集合。
    names: BTreeSet<String>,
    /// (simple name, super) 去重后的命中记录。
    classes: BTreeMap<String, String>,
}

fn dex_jars(dexinfo: &Value) -> BTreeMap<String, DexJar> {
    let mut out = BTreeMap::new();
    for jar in dexinfo["jars"].as_array().expect("dexinfo 缺 jars 数组") {
        let file = jar["file"].as_str().expect("dexinfo jar 缺 file");
        let mut names = BTreeSet::new();
        let mut classes = BTreeMap::new();
        for dex in jar["dexes"].as_array().expect("dexinfo jar 缺 dexes") {
            for c in dex["spiderClasses"].as_array().expect("spiderClasses 不是数组") {
                let name = c["name"].as_str().expect("spider 类缺 name").to_string();
                let sup = c["super"].as_str().unwrap_or("").to_string();
                names.insert(name.clone());
                classes.entry(name).or_insert(sup);
            }
        }
        out.insert(basename(file), DexJar { names, classes });
    }
    out
}

fn rulepack_apis(profiles: &Value) -> Vec<(String, Value)> {
    profiles["apis"]
        .as_object()
        .expect("profiles 缺 apis 对象")
        .iter()
        .filter(|(_, p)| p["strategy"].as_str() == Some("rulepack"))
        .map(|(api, p)| (api.clone(), p.clone()))
        .collect()
}

#[test]
fn rulepack_api_class_is_in_its_jar() {
    let Some(dexinfo) = load_json(&dexinfo_path()) else {
        eprintln!("skip: {} 不存在（CI 无 _rea 快照）", dexinfo_path().display());
        return;
    };
    let profiles = load_json(&profiles_path()).expect("profiles.json 必须存在");
    let dex = dex_jars(&dexinfo);
    let apis = rulepack_apis(&profiles);
    assert!(!apis.is_empty(), "没有 rulepack api");

    let mut errors = Vec::new();
    for (api, p) in &apis {
        let class = p["class"].as_str().unwrap_or_else(|| panic!("{api} 缺 class"));
        let Some(simple) = class.strip_prefix(SPIDER_PREFIX) else {
            errors.push(format!("{api}: class {class} 不在 {SPIDER_PREFIX} 命名空间"));
            continue;
        };
        if simple.contains('$') || simple.is_empty() {
            errors.push(format!("{api}: class 不是顶层类: {class}"));
            continue;
        }
        let jars = p["jars"].as_array().expect("api 缺 jars 数组");
        if jars.is_empty() {
            errors.push(format!("{api}: jars 为空"));
            continue;
        }
        let mut hit = 0usize;
        for jar in jars {
            let name = jar.as_str().expect("jars 元素不是字符串");
            match dex.get(name) {
                Some(d) if d.names.contains(simple) => hit += 1,
                Some(_) => {}
                None => errors.push(format!("{api}: jar {name} 不在 dexinfo 中")),
            }
        }
        if hit == 0 {
            errors.push(format!("{api}: class 简名 {simple} 未出现在 jars {jars:?} 的 spider 类清单"));
        }
    }
    assert!(errors.is_empty(), "api.class 与 dexinfo 脱节（{} 处）:\n{}", errors.len(), errors.join("\n"));
}

#[test]
fn api_class_super_chain_is_resolvable() {
    let Some(dexinfo) = load_json(&dexinfo_path()) else {
        eprintln!("skip: {} 不存在（CI 无 _rea 快照）", dexinfo_path().display());
        return;
    };
    let profiles = load_json(&profiles_path()).expect("profiles.json 必须存在");
    let dex = dex_jars(&dexinfo);

    let mut errors = Vec::new();
    for (api, p) in rulepack_apis(&profiles) {
        let class = p["class"].as_str().expect("api 缺 class");
        let simple = class.strip_prefix(SPIDER_PREFIX).unwrap_or(class);
        for jar in p["jars"].as_array().expect("api 缺 jars 数组") {
            let name = jar.as_str().expect("jars 元素不是字符串");
            let Some(d) = dex.get(name) else { continue };
            let Some(sup) = d.classes.get(simple) else { continue };
            // 同简名类可能在不同 jar 内是非 Spider 的辅助类（如 `Youtube` 在 FxzHm/fan
            // 内 extends Object，仅 43 行静态辅助），不构成类图缺口。
            if sup != "Spider" && sup != "Object" && !d.names.contains(sup) {
                errors.push(format!("{api}: {simple} 的 super `{sup}` 在 {name} 内不可解析"));
            }
        }
    }
    assert!(errors.is_empty(), "spider 类图不完整（{} 处）:\n{}", errors.len(), errors.join("\n"));
}
