//! 判据 66：`profiles.json` 的**磁盘原文 schema**——字段存在性、类型与策略专属字段。
//!
//! `ProfilesFile` 的绝大多数字段带 `#[serde(default)]`：字段名拼错、整块缺失或类型被改成
//! 宽松形态时，反序列化不会失败，只会静默取默认值，再被后续判据当成「合法数据」。
//! 判据 61 只覆盖头部包络与 totals，本判据直接读原始 JSON，逐条锁定：
//!
//! - 每个 api 必备 8 个字段（class/configs/guarded/jars/methods/sites/strategy + reason|blockedReason）；
//! - `reason` 与 `blockedReason` 恰好一个（unported 用 blockedReason，其余用 reason）；
//! - `difficulty` 只在 rulepack/unported/unsupported 出现，rulepack 必须出现，script 禁止出现；
//! - 所有字段类型正确、不许有未知字段（拼写错误即红）；
//! - jar 条目恰好 6 个字段；totals / strategies / 顶层字段集合精确。
//!
//! 本判据只读仓库内文件，CI 不软跳过。

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

fn profiles_value() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/profiles.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取画像原文失败 {}: {e}", path.display()));
    serde_json::from_str(&text).expect("profiles.json 不是合法 JSON")
}

fn keys(value: &Value) -> BTreeSet<String> {
    value.as_object().expect("不是对象").keys().cloned().collect()
}

fn str_field<'a>(entry: &'a Value, api: &str, field: &str) -> &'a str {
    entry[field]
        .as_str()
        .unwrap_or_else(|| panic!("api {api} 的 {field} 不是字符串"))
}

#[test]
fn api_entries_match_strategy_schema() {
    let root = profiles_value();
    let apis = root["apis"].as_object().expect("apis 不是对象");
    assert_eq!(apis.len(), 214, "api 数应为 214");

    let common: BTreeSet<String> = [
        "class", "configs", "guarded", "jars", "methods", "sites", "strategy",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let mut errors = Vec::new();
    for (api, entry) in apis {
        let strategy = str_field(entry, api, "strategy");
        let mut allowed = common.clone();
        let mut required = common.clone();
        let difficulty_allowed = matches!(strategy, "rulepack" | "unported" | "unsupported");
        match strategy {
            "rulepack" => {
                allowed.insert("reason".into());
                required.insert("reason".into());
                allowed.insert("difficulty".into());
                required.insert("difficulty".into());
            }
            "script" => {
                allowed.insert("reason".into());
                required.insert("reason".into());
            }
            "unsupported" => {
                allowed.insert("reason".into());
                required.insert("reason".into());
                allowed.insert("difficulty".into());
            }
            "unported" => {
                allowed.insert("blockedReason".into());
                required.insert("blockedReason".into());
                allowed.insert("difficulty".into());
            }
            other => {
                errors.push(format!("{api}: 未知 strategy {other:?}"));
                continue;
            }
        }
        let present = keys(entry);
        let extra: Vec<&String> = present.difference(&allowed).collect();
        let missing: Vec<&String> = required.difference(&present).collect();
        if !extra.is_empty() {
            errors.push(format!("{api}: 出现未知字段 {extra:?}（拼写错误或以 serde default 掩盖）"));
        }
        if !missing.is_empty() {
            errors.push(format!("{api}: 缺少字段 {missing:?}"));
        }
        // 类型
        if !entry["sites"].is_u64() || entry["sites"].as_u64().unwrap_or(0) == 0 {
            errors.push(format!("{api}: sites 必须是正整数"));
        }
        if !entry["methods"].is_u64() {
            errors.push(format!("{api}: methods 必须是整数"));
        }
        if !entry["guarded"].is_boolean() {
            errors.push(format!("{api}: guarded 必须是布尔"));
        }
        if !entry["jars"].is_array() {
            errors.push(format!("{api}: jars 必须是数组"));
        }
        if !entry["configs"].is_array() {
            errors.push(format!("{api}: configs 必须是数组"));
        }
        if let Some(class) = entry["class"].as_str() {
            if !class.starts_with("com.github.catvod.spider.") {
                errors.push(format!("{api}: class 前缀不合契约 {class}"));
            }
        }
        // reason / blockedReason 恰好一个
        let has_reason = present.contains("reason");
        let has_blocked = present.contains("blockedReason");
        if has_reason == has_blocked {
            errors.push(format!("{api}: reason 与 blockedReason 必须恰好一个"));
        }
        if strategy == "unported" && !has_blocked {
            errors.push(format!("{api}: unported 缺少 blockedReason"));
        }
        if strategy != "unported" && !has_reason {
            errors.push(format!("{api}: {strategy} 缺少 reason"));
        }
        if let Some(reason) = entry["reason"].as_str() {
            if reason.trim().is_empty() {
                errors.push(format!("{api}: reason 为空"));
            }
        }
        if let Some(reason) = entry["blockedReason"].as_str() {
            if reason.trim().is_empty() {
                errors.push(format!("{api}: blockedReason 为空"));
            }
        }
        // 难度
        match entry["difficulty"].as_str() {
            Some(level) => {
                if !difficulty_allowed {
                    errors.push(format!("{api}: strategy={strategy} 不应出现 difficulty"));
                } else if !matches!(level, "L1" | "L2" | "L3" | "L4") {
                    errors.push(format!("{api}: 非法难度 {level:?}"));
                }
            }
            None => {
                if strategy == "rulepack" {
                    errors.push(format!("{api}: rulepack 必须带 difficulty"));
                }
            }
        }
    }
    assert!(errors.is_empty(), "api schema 偏差：\n{}", errors.join("\n"));
}

#[test]
fn jar_and_totals_entries_have_exact_shape() {
    let root = profiles_value();
    let top = keys(&root);
    let expected_top: BTreeSet<String> = ["apis", "generatedAt", "jars", "schema", "source", "totals"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(top, expected_top, "画像顶层字段集合不一致");

    let totals = &root["totals"];
    let totals_keys = keys(totals);
    let expected_totals: BTreeSet<String> = ["sites", "distinctApis", "strategies", "jars"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(totals_keys, expected_totals, "totals 字段集合不一致");
    let strategies = keys(&totals["strategies"]);
    let expected_strategies: BTreeSet<String> =
        ["rulepack", "unported", "script", "unsupported"].iter().map(|s| s.to_string()).collect();
    assert_eq!(strategies, expected_strategies, "strategies 字段集合不一致");
    assert_eq!(totals["sites"].as_u64(), Some(612));
    assert_eq!(totals["distinctApis"].as_u64(), Some(214));
    assert_eq!(totals["jars"].as_u64(), Some(20));

    let jars = root["jars"].as_array().expect("jars 不是数组");
    assert_eq!(jars.len(), 20, "jar 数应为 20");
    let expected_jar: BTreeSet<String> = ["bytes", "classes", "guarded", "name", "sha256", "spiders"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut errors = Vec::new();
    for jar in jars {
        let present = keys(jar);
        if present != expected_jar {
            errors.push(format!(
                "jar {:?}: 字段集合不一致（多 {:?} / 少 {:?}）",
                jar["name"],
                present.difference(&expected_jar).collect::<Vec<_>>(),
                expected_jar.difference(&present).collect::<Vec<_>>()
            ));
        }
        let name = jar["name"].as_str().unwrap_or_default();
        let sha = jar["sha256"].as_str().unwrap_or_default();
        if name.is_empty() {
            errors.push("jar: name 为空".to_string());
        }
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            errors.push(format!("jar {name}: sha256 不是 64 位 hex"));
        }
        for field in ["bytes", "classes", "spiders"] {
            if !jar[field].is_u64() {
                errors.push(format!("jar {name}: {field} 不是整数"));
            }
        }
        if !jar["guarded"].is_boolean() {
            errors.push(format!("jar {name}: guarded 不是布尔"));
        }
    }
    assert!(errors.is_empty(), "jar schema 偏差：\n{}", errors.join("\n"));
}
