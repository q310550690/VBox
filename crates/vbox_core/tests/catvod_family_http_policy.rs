//! 判据 70：原生家族的 HTTP 客户端必须走宿主的统一策略层。
//!
//! `crate::http::client_for(rules)` 负责超时、代理、UA 与全局规则；`client_for_insecure`
//! 专门为「站点证书过期」准备，会关闭 TLS 校验。如果某个家族自己 `reqwest::Client::new()`
//! 或 `builder()`，就会静默绕过超时/代理/证书策略；如果 `client_for_insecure` 扩散到别的
//! 家族，用户会在不知情的情况下对所有站点关闭证书校验。两种漂移都没有编译错误，也没有
//! 任何既有判据覆盖。
//!
//! 本判据锁三件事：
//! 1. `families/` 下除 `mod.rs` 外，凡是有网络行为的家族必须引用 `client_for(`；
//! 2. 家族文件不得直接构造 `reqwest::Client`（或出现 `danger_accept_invalid_certs`）；
//! 3. `client_for_insecure` 只允许出现在 `yinyuetai`（站点证书已过期、文档记录的安全偏差）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn families_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/families")
}

fn family_files() -> Vec<(String, PathBuf)> {
    let dir = families_dir();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs") {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if name != "mod" {
                out.push((name, path));
            }
        }
    }
    assert!(out.len() >= 65, "家族文件数量异常: {}", out.len());
    out
}

/// 无直接 HTTP 调用的家族：Huban 是「配置引导壳」，所有入口只返回占位分类/详情；
/// Youtube 走 `yt-dlp` 子进程桥（§262），二者都不直接构造 HTTP 请求。
const NO_NETWORK_FAMILIES: &[&str] = &["huban", "youtube"];

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn network_families_use_shared_client_helper() {
    let mut missing = Vec::new();
    for (name, path) in family_files() {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if NO_NETWORK_FAMILIES.contains(&name.as_str()) {
            assert!(
                !text.contains("client_for("),
                "无网络家族 {name} 不应引用 client_for，若它开始联网请移出白名单"
            );
            continue;
        }
        // yinyuetai 只走 client_for_insecure；它仍属于统一策略层（见第三项测试）。
        if !text.contains("client_for(") && !text.contains("client_for_insecure(") {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "以下家族未使用统一客户端 client_for（会绕过超时/代理/证书策略）：{missing:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn families_never_build_raw_reqwest_clients() {
    let mut offenders = Vec::new();
    for (name, path) in family_files() {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for needle in [
            "reqwest::Client::new(",
            "reqwest::Client::builder(",
            "danger_accept_invalid_certs",
        ] {
            if text.contains(needle) {
                offenders.push(format!("{name}: {needle}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "家族不得绕过统一 HTTP 策略（请改用 http::client_for）：{offenders:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn insecure_tls_is_scoped_to_yinyuetai_only() {
    let mut users = BTreeSet::new();
    for (name, path) in family_files() {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.contains("client_for_insecure(") {
            users.insert(name);
        }
    }
    let expected: BTreeSet<String> = ["yinyuetai".to_string()].into_iter().collect();
    assert_eq!(
        users, expected,
        "关闭 TLS 校验的家族必须只有 yinyuetai（站点证书过期）；新增用户须在计划文档记录安全偏差"
    );
}
