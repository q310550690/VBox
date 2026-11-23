//! 判据 57：画像 api 的 `configs`（来源配置标签）必须与站点清单逐 api 闭合。
//!
//! 为什么需要：画像里 `apis[key].sites` 与 `apis[key].configs` 都是派生值，
//! 站点清单 `sites.json` 才是源数据。此前两者各自生成，若某一侧漏改（新增站点
//! 忘了改 configs、或 api 键带尾空格导致配置统计串台），画像表面上自洽，实际
//! 已与清单脱节。本判据按 api 把「站点数」与「来源标签集合」双向锁死。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/vbox_core 应位于仓库两级目录下")
        .to_path_buf()
}

fn load_json(path: &Path) -> Option<serde_json::Value> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(serde_json::from_str(&text).unwrap_or_else(|e| {
            panic!("{} 不是合法 JSON: {e}", path.display())
        })),
        Err(_) => None,
    }
}

/// `dc(xhztv)(http://xhztv.top/xhz)` -> `dc(xhztv)`
///
/// 站点清单的 `repo` 形如「模式(维护者)(配置地址)」，画像 `configs` 只保留前两段。
fn repo_label(repo: &str) -> Option<&str> {
    let first = repo.find('(')?;
    let close = repo[first + 1..].find(')')? + first + 1;
    // 站点清单的 repo 通常已是标签本身（dc(xhztv)）；兼容旧格式
    // 「模式(维护者)(配置地址)」，只取前两段。
    match repo[close + 1..].find('(') {
        Some(rel) => Some(&repo[..close + 1 + rel]),
        None => Some(repo),
    }
}

fn sites_of<'a>(
    sites: &'a [serde_json::Value],
) -> BTreeMap<String, Vec<&'a serde_json::Value>> {
    let mut map: BTreeMap<String, Vec<&serde_json::Value>> = BTreeMap::new();
    for site in sites {
        let api = site
            .get("api")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        assert!(!api.is_empty(), "站点 api 不得为空: {site}");
        map.entry(api).or_default().push(site);
    }
    map
}

fn string_set(value: Option<&serde_json::Value>, what: &str) -> BTreeSet<String> {
    let array = value
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{what} 应为数组"));
    array
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("{what} 中存在非字符串项"))
                .to_string()
        })
        .collect()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn api_configs_match_site_repo_labels() {
    let root = repo_root();
    let profiles_path = root.join("crates/vbox_core/src/spider/catvod/profiles.json");
    let sites_path = root.join("_rea/samples/inventory/sites.json");
    let profiles = load_json(&profiles_path).expect("profiles.json 必须存在");
    let Some(sites_json) = load_json(&sites_path) else {
        eprintln!("_rea 不在当前检出，跳过判据 57（CI 软跳过）");
        return;
    };
    let sites_array = sites_json
        .get("sites")
        .and_then(|v| v.as_array())
        .or_else(|| sites_json.as_array())
        .expect("sites.json 应为数组或含 sites 数组");
    let by_api = sites_of(sites_array);
    let apis = profiles
        .get("apis")
        .and_then(|v| v.as_object())
        .expect("profiles.apis 应为对象");

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (api, meta) in apis {
        let key = api.trim();
        let sites = by_api.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let declared = meta
            .get("sites")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| panic!("{api} 缺 sites 字段"));
        if declared != sites.len() as u64 {
            failures.push(format!(
                "{api}: 画像 sites={declared}，清单实际={}",
                sites.len()
            ));
        }

        let configs = string_set(meta.get("configs"), &format!("{api}.configs"));
        if configs.is_empty() {
            failures.push(format!("{api}: configs 为空"));
        }
        let derived: BTreeSet<String> = sites
            .iter()
            .filter_map(|s| s.get("repo").and_then(|v| v.as_str()))
            .filter_map(repo_label)
            .map(str::to_string)
            .collect();
        let missing: Vec<&String> = derived.difference(&configs).collect();
        let extra: Vec<&String> = configs.difference(&derived).collect();
        if !missing.is_empty() || !extra.is_empty() {
            failures.push(format!(
                "{api}: 清单有而 configs 缺 {missing:?}；configs 有而清单缺 {extra:?}"
            ));
        }
        checked += 1;
    }

    assert!(
        checked >= 200,
        "本次只核对 {checked} 个 api，画像疑似被截断"
    );
    assert!(
        failures.is_empty(),
        "api→configs 与站点清单不一致（{} 处）：\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!("判据 57：{checked} 个 api 的 configs 与站点 repo 标签逐项一致");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn every_site_repo_label_is_covered_by_its_api() {
    let root = repo_root();
    let profiles_path = root.join("crates/vbox_core/src/spider/catvod/profiles.json");
    let sites_path = root.join("_rea/samples/inventory/sites.json");
    let profiles = load_json(&profiles_path).expect("profiles.json 必须存在");
    let Some(sites_json) = load_json(&sites_path) else {
        eprintln!("_rea 不在当前检出，跳过判据 57（CI 软跳过）");
        return;
    };
    let sites_array = sites_json
        .get("sites")
        .and_then(|v| v.as_array())
        .or_else(|| sites_json.as_array())
        .expect("sites.json 应为数组或含 sites 数组");
    let apis = profiles
        .get("apis")
        .and_then(|v| v.as_object())
        .expect("profiles.apis 应为对象");

    let mut failures: Vec<String> = Vec::new();
    for site in sites_array {
        let key = site
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("<no-key>");
        let api = site
            .get("api")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let Some(meta) = apis.get(api) else {
            failures.push(format!("{key}: api {api:?} 未出现在画像 apis 中"));
            continue;
        };
        let Some(repo) = site.get("repo").and_then(|v| v.as_str()) else {
            failures.push(format!("{key}: 缺 repo 字段"));
            continue;
        };
        let Some(label) = repo_label(repo) else {
            failures.push(format!("{key}: repo 形状不可解析: {repo:?}"));
            continue;
        };
        let configs = string_set(meta.get("configs"), &format!("{api}.configs"));
        if !configs.contains(label) {
            failures.push(format!("{key}: repo 标签 {label:?} 不在 {api} 的 configs 中"));
        }
    }

    assert!(
        sites_array.len() >= 600,
        "本次只读到 {} 个站点，清单疑似被截断",
        sites_array.len()
    );
    assert!(
        failures.is_empty(),
        "站点→api→configs 闭合失败（{} 处）：\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!(
        "判据 57：{} 个站点全部落到画像 api 且 repo 标签被该 api 的 configs 覆盖",
        sites_array.len()
    );
}
