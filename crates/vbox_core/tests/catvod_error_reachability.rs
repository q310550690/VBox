//! 判据 58：错误码不仅「定义齐」，还要「生产可达」且「码可回环」。
//!
//! 为什么需要：`ErrorClass` 是面向用户与源健康统计的唯一错误语言。此前
//! `catvod_error_taxonomy` 只校验 20 个变体的码前缀/唯一/hint，`catvod_error_paths`
//! 只覆盖若干真实路径——如果某个变体从没在生产代码里被构造（只剩 `code()`/tests
//! 引用），它就是一个永远不会出现的「僵尸错误码」；反过来，若两个变体的码互为
//! 前缀，`detect()` 的「最长匹配」会让短码永远无法还原。
//!
//! 两条约束：
//! 1. 每个变体在 `crates/vbox_core/src` 的生产代码（去掉 `#[cfg(test)]` 之后）
//!    至少有一处 `ErrorClass::Variant` 构造；
//! 2. 每个变体的 `code()` 放进一条消息后，`detect()` 必须还原出同一变体。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vbox_core::spider::catvod::contract::ErrorClass;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/vbox_core 应位于仓库两级目录下")
        .to_path_buf()
}

fn walk_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("读取 {}: {e}", dir.display())) {
        let path = entry.expect("目录项").path();
        if path.is_dir() {
            walk_rs(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// 生产代码前缀：约定 `#[cfg(test)]` 模块位于文件末尾，截断后即不含测试构造。
fn production_part(text: &str) -> &str {
    match text.find("#[cfg(test)]") {
        Some(idx) => &text[..idx],
        None => text,
    }
}

fn enum_variants(contract: &str) -> Vec<String> {
    let marker = "pub enum ErrorClass {";
    let start = contract
        .find(marker)
        .unwrap_or_else(|| panic!("contract.rs 中找不到 {marker}"))
        + marker.len();
    let rest = &contract[start..];
    let mut depth = 1usize;
    let mut end = rest.len();
    for (idx, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = idx;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &rest[..end];
    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("//") {
                return None;
            }
            let name: String = line
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

fn count_constructions(text: &str, variant: &str) -> usize {
    let needle = format!("ErrorClass::{variant}");
    let mut count = 0;
    for (idx, _) in text.match_indices(&needle) {
        let after = &text[idx + needle.len()..];
        // 边界：Network 不能命中 NetworkDns。
        if after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        // match 模式（`ErrorClass::X => ...`）不是构造。
        if after.trim_start().starts_with("=>") {
            continue;
        }
        // use 列表不是构造。
        if text[..idx].trim_end().ends_with("use") {
            continue;
        }
        count += 1;
    }
    count
}

#[test]
fn every_error_class_has_production_construction() {
    let root = repo_root();
    let contract_path = root.join("crates/vbox_core/src/spider/catvod/contract.rs");
    let contract = std::fs::read_to_string(&contract_path)
        .unwrap_or_else(|e| panic!("读取 {}: {e}", contract_path.display()));
    let variants = enum_variants(&contract);
    assert!(
        variants.len() >= 20,
        "ErrorClass 只解析出 {} 个变体，枚举疑似被截断",
        variants.len()
    );
    assert_eq!(
        ErrorClass::ALL.len(),
        variants.len(),
        "ErrorClass::ALL 与枚举定义数量不一致"
    );

    let mut files = Vec::new();
    walk_rs(&root.join("crates/vbox_core/src"), &mut files);
    let mut counts: BTreeMap<String, usize> = variants.iter().map(|v| (v.clone(), 0)).collect();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let text = production_part(&text);
        for variant in &variants {
            *counts.get_mut(variant).expect("变体键") += count_constructions(text, variant);
        }
    }

    let missing: Vec<String> = counts
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(v, _)| v.clone())
        .collect();
    assert!(
        missing.is_empty(),
        "以下错误变体在生产代码中没有构造点（僵尸码）：{missing:?}"
    );
    println!(
        "判据 58：{} 个 ErrorClass 变体全部在生产代码中可达（扫描 {} 个 .rs 文件）",
        variants.len(),
        files.len()
    );
}

#[test]
fn error_codes_survive_detect_roundtrip() {
    let mut codes = BTreeMap::new();
    for class in ErrorClass::ALL.iter().copied() {
        let code = class.code();
        assert!(
            code.starts_with("catvod."),
            "{code} 不以 catvod. 开头"
        );
        if let Some(prev) = codes.insert(code.to_string(), class) {
            panic!("错误码重复：{code} 同时属于 {prev:?} 与 {class:?}");
        }
        let message = format!("上游请求失败：{code}（站点 demo）");
        assert_eq!(
            ErrorClass::detect(&message),
            Some(class),
            "码 {code} 放入消息后 detect 还原失败（可能被更长的前缀码遮蔽）"
        );
    }
    assert_eq!(codes.len(), ErrorClass::ALL.len());
    println!(
        "判据 58：{} 个错误码唯一且 detect 回环全部一致",
        codes.len()
    );
}
