//! 判据 56：家族模块 ↔ 接管 api ↔ 专属测试文件的**单点**闭环。
//!
//! 判据 47/REA 索引保证「每个家族有 REA 附录与测试文件名」，但它只看**集合级**：
//! 一个从别的家族复制来的测试文件、或一个只测 `registry` 而不触碰本家族 api 的
//! 测试文件，都能让集合级判据保持绿色。判据 56 从源码静态解析：
//!
//!   * `families/mod.rs` 声明的每个模块，其 `handles()` 必须拥有至少一个
//!     `csp_*` api（字面量或 `API` 常量）；
//!   * 该家族的专属测试文件必须出现至少一个**它自己拥有**的 api 字面量；
//!   * 每个 `strategy=rulepack` 的画像 api 必须被恰好一个模块 `handles` 接管
//!     （`csp_XBPQ` 是原生规则引擎、不是家族，明确豁免）；
//!   * `handles` 声明的 api 必须全部存在于画像中（没有死分派）。
//!
//! 这里不重复运行时 dispatch 测试（`catvod_dispatch_wiring`），而是把「测试文件
//! 真的指到它该测的 api」钉住——这是纯文本判据无法从集合计数里看出来的。
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates 目录")
        .parent()
        .expect("仓库根")
        .to_path_buf()
}

fn families_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/families")
}

fn tests_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// 读取 `families/mod.rs` 中 `pub mod <name>;` 的声明顺序（即 family_of 的匹配顺序）。
fn declared_modules() -> Vec<String> {
    let raw = fs::read_to_string(families_dir().join("mod.rs")).expect("families/mod.rs 必须存在");
    let mut out = Vec::new();
    for line in raw.lines() {
        let Some(rest) = line.strip_prefix("pub mod ") else { continue };
        let Some(name) = rest.strip_suffix(';') else { continue };
        let name = name.trim();
        if !name.is_empty() {
            out.push(name.to_string());
        }
    }
    assert!(!out.is_empty(), "mod.rs 未声明任何家族模块");
    out
}

fn balanced_body<'a>(src: &'a str, anchor: &str) -> &'a str {
    let start = src.find(anchor).unwrap_or_else(|| panic!("未找到 {anchor}"));
    let open = src[start..].find('{').expect("函数体缺少 {") + start;
    let mut depth = 0usize;
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("{anchor} 函数体花括号不平衡");
}

fn quoted_csp(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'"' {
            end += 1;
        }
        if end >= bytes.len() {
            break;
        }
        let lit = &body[start..end];
        if lit.starts_with("csp_") && lit.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.insert(lit.to_string());
        }
        i = end + 1;
    }
    out
}

/// `handles()` 直接写的字面量，加上 `handles()` 引用到的 `const NAME: &str = "csp_X"`。
fn handled_apis(module: &str) -> BTreeSet<String> {
    let src = fs::read_to_string(families_dir().join(format!("{module}.rs")))
        .unwrap_or_else(|e| panic!("读取 {module}.rs 失败: {e}"));
    let body = balanced_body(&src, "pub fn handles");
    let mut out = quoted_csp(body);

    // 常量式 handlers：`pub const API: &str = "csp_X";` 在 handles 中被引用。
    let mut consts = BTreeMap::new();
    for (idx, line) in src.lines().enumerate() {
        let Some(pos) = line.find("const ") else { continue };
        let rest = &line[pos + 6..];
        let Some(eq) = rest.find('=') else { continue };
        let name = rest[..eq].split(':').next().unwrap_or("").trim();
        let tail = &rest[eq + 1..];
        if !name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        if let Some(lit) = tail.split('"').nth(1) {
            if lit.starts_with("csp_") {
                consts.insert(name.to_string(), lit.to_string());
            }
        }
        let _ = idx;
    }
    for (name, lit) in consts {
        if body.contains(&name) {
            out.insert(lit);
        }
    }
    assert!(!out.is_empty(), "{module}: handles() 没有拥有任何 csp_* api");
    out
}

fn test_file(module: &str) -> PathBuf {
    tests_dir().join(format!("catvod_{module}_family.rs"))
}

#[test]
fn family_module_has_owned_api_referenced_by_its_test() {
    let modules = declared_modules();
    let mut errors = Vec::new();
    let mut declared_files = BTreeSet::new();
    for module in &modules {
        let path = families_dir().join(format!("{module}.rs"));
        if !path.is_file() {
            errors.push(format!("{module}: mod.rs 声明了模块但 families/{module}.rs 不存在"));
            continue;
        }
        declared_files.insert(format!("{module}.rs"));
        let owned = handled_apis(module);
        let tf = test_file(module);
        let Ok(text) = fs::read_to_string(&tf) else {
            errors.push(format!("{module}: 缺少专属测试 {}", tf.display()));
            continue;
        };
        if !owned.iter().any(|api| text.contains(api)) {
            errors.push(format!(
                "{module}: 测试文件没有出现本家族任何 api 字面量（拥有 {} 个：{:?}）",
                owned.len(),
                owned
            ));
        }
        for api in &owned {
            let owner = vbox_core::spider::catvod::families::family_of(api);
            if owner != Some(module.as_str()) {
                errors.push(format!(
                    "{module}: handles 声明 {api}，但 family_of 返回 {owner:?}（分派被遮蔽或大小写不一致）"
                ));
            }
        }
    }
    // 反查：磁盘上有家族文件却未在 mod.rs 声明。
    if let Ok(entries) = fs::read_dir(families_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".rs") && name != "mod.rs" && !declared_files.contains(&name) {
                errors.push(format!("families/{name} 存在但未在 mod.rs 声明"));
            }
        }
    }
    assert!(errors.is_empty(), "家族/测试单点闭环断裂（{} 处）:\n{}", errors.len(), errors.join("\n"));
}

#[test]
fn every_rulepack_api_is_owned_by_exactly_one_family() {
    let modules = declared_modules();
    // 小写 api → 拥有它的模块集合（同一模块重复声明只算一次）。
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for module in &modules {
        for api in handled_apis(module) {
            owners.entry(api.to_ascii_lowercase()).or_default().insert(module.clone());
        }
    }
    let mut errors = Vec::new();
    for (api, mods) in &owners {
        if mods.len() > 1 {
            errors.push(format!("{api} 被多个家族同时 handles：{mods:?}（分派歧义）"));
        }
    }

    let raw = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/profiles.json"))
        .expect("profiles.json 必须存在");
    let profiles: serde_json::Value = serde_json::from_str(&raw).expect("profiles.json 不是合法 JSON");
    let apis = profiles["apis"].as_object().expect("profiles 缺 apis 对象");
    for (api, meta) in apis {
        let owners_of = owners.get(&api.to_ascii_lowercase());
        if let Some(mods) = owners_of {
            if !mods.is_empty() {
                continue;
            }
        }
        // 原生规则引擎不是家族实现，明确豁免：
        //   csp_XBPQ        —— XBPQ 规则包（xpath_rule 引擎）
        //   csp_XYQHiker*   —— XYQHiker 规则包引擎（含 AL 变体）
        if matches!(api.as_str(), "csp_XBPQ" | "csp_XYQHiker" | "csp_XYQHikerAL") {
            continue;
        }
        if meta["strategy"].as_str() == Some("rulepack") {
            errors.push(format!("rulepack api {api} 没有任何家族 handles 接管"));
        }
    }
    // 死分派：handles 声明了画像中不存在的 api。
    let profile_keys: BTreeSet<String> = apis.keys().map(|k| k.to_ascii_lowercase()).collect();
    for api in owners.keys() {
        if !profile_keys.contains(api) {
            errors.push(format!("handles 声明了画像中不存在的 api: {api}"));
        }
    }
    assert!(errors.is_empty(), "api 归属闭环断裂（{} 处）:\n{}", errors.len(), errors.join("\n"));
}
