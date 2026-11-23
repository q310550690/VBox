//! 兼容性遥测（L1 元数据，本地 spool）。
//!
//! 设计约束（见 `docs/type3兼容/遥测与反馈设计.md`）：
//! - **只在本机落盘，默认不上传**；导出诊断包 / 生成 GitHub Issue 由用户显式触发；
//! - 不含站点配置、URL、cookie、ext 等用户数据；
//! - 每只 jar 只记录一次（按内容 sha256 去重），失败事件按追加日志保留（有上限）。
//!
//! 远端上报（Worker 端点）预留 `endpoint()`/`flush()`：`VBOX_TELEMETRY_ENDPOINT` 未配置时为空操作。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// 报告 schema 版本（字段变更时递增）。
pub const SCHEMA: u32 = 1;
/// 失败事件日志上限（超出后轮转成 `events.1.jsonl`）。
pub const MAX_EVENTS_BYTES: u64 = 1_048_576;
/// 单条 detail 的最大字符数（脱敏后）。
pub const MAX_DETAIL: usize = 800;
/// stderr 尾部保留的最大字符数。
pub const MAX_STDERR_TAIL: usize = 4096;

/// 一只 jar 的 L1 元数据报告（不含 jar 内容）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JarReport {
    pub schema: u32,
    pub install_id: String,
    pub created_at: u64,
    pub app_version: String,
    pub jvm_rev: Option<String>,
    pub os: String,
    pub arch: String,
    pub jar_sha256: String,
    pub jar_size: u64,
    pub family: String,
    pub markers: Vec<String>,
    pub native_libs: Vec<String>,
    pub suffixes: Vec<String>,
    pub needs_native_crypto: bool,
}

/// 一次失败事件（错误码 + 脱敏后的详情开头）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FailureEvent {
    pub at: u64,
    pub code: String,
    pub detail: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 数据根目录：`VBOX_DATA_DIR` → `%LOCALAPPDATA%/VBox` → 系统临时目录。
pub fn data_root() -> PathBuf {
    if let Ok(dir) = std::env::var("VBOX_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.trim().is_empty() {
            return PathBuf::from(local).join("VBox");
        }
    }
    std::env::temp_dir().join("vbox")
}

/// 遥测目录（本机 spool）。
pub fn dir() -> PathBuf {
    data_root().join("telemetry")
}

/// 匿名安装 id：首次调用生成并落盘，之后复用（不含任何可识别信息）。
pub fn install_id() -> String {
    let path = dir().join("install-id");
    if let Ok(text) = fs::read_to_string(&path) {
        let text = text.trim();
        if text.len() >= 16 {
            return text.to_string();
        }
    }
    // 无 RNG 依赖：用时间 + pid + 栈地址的 sha256 前 16 字节，足够做去重键。
    let seed = format!(
        "{}:{}:{:p}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        std::process::id(),
        &path
    );
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    let id = hex16(&digest);
    let _ = fs::create_dir_all(dir());
    let _ = fs::write(&path, &id);
    id
}

fn hex16(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(32);
    for b in bytes.iter().take(16) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 把可能含隐私的文本脱敏：去掉 URL 的 query/fragment、常见凭据参数、控制字符，并截断。
pub fn sanitize(text: &str, limit: usize) -> String {
    let mut cleaned = String::with_capacity(text.len().min(limit * 2));
    for raw in text.lines() {
        let mut line = raw.to_string();
        // 去掉 ANSI 转义（日志里偶尔有）。
        while let Some(i) = line.find('\u{1b}') {
            let rest = &line[i..];
            let end = rest.find('m').map(|p| i + p + 1).unwrap_or(line.len());
            line.replace_range(i..end, "");
        }
        // URL 的 query/fragment 一律丢弃（token/cookie 通常在这里）。
        if let Some(q) = line.find('?') {
            line.truncate(q);
        }
        if let Some(h) = line.find('#') {
            line.truncate(h);
        }
        // 常见凭据键：`key=value` / `Key: value` / `Authorization: Bearer xxx` 一律抹成 ***。
        // 头类键（authorization/cookie）抹到行尾，其余键抹到空白或分隔符。
        for key in [
            "authorization",
            "cookie",
            "set-cookie",
            "access_token",
            "token",
            "password",
            "passwd",
            "secret",
            "api_key",
            "apikey",
        ] {
            let lower = line.to_ascii_lowercase();
            let Some(pos) = lower.find(key) else { continue };
            let mut start = pos + key.len();
            while let Some(c) = line[start..].chars().next() {
                if c == '=' || c == ':' || c == ' ' || c == '"' || c == '\'' {
                    start += c.len_utf8();
                } else {
                    break;
                }
            }
            if start >= line.len() {
                continue;
            }
            let to_eol = key == "authorization" || key == "cookie" || key == "set-cookie";
            let end = if to_eol {
                line.len()
            } else {
                line[start..]
                    .find(|c: char| c.is_whitespace() || c == '&' || c == '"' || c == '\'')
                    .map(|p| start + p)
                    .unwrap_or(line.len())
            };
            line.replace_range(start..end, "***");
        }
        cleaned.push_str(line.trim_end());
        cleaned.push('\n');
        if cleaned.chars().count() >= limit {
            break;
        }
    }
    let mut out: String = cleaned.chars().take(limit).collect();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// 记录一只 jar 的 L1 元数据（按 sha256 去重：同一只 jar 只写一次）。
///
/// 返回新写入的文件路径；已记录过（或 sha 为空）返回 `None`。
pub fn note_jar_once(
    jar: &Path,
    sha: &str,
    fp: &crate::ShellFingerprint,
) -> Option<PathBuf> {
    if sha.is_empty() {
        return None;
    }
    let jars_dir = dir().join("jars");
    if fs::create_dir_all(&jars_dir).is_err() {
        return None;
    }
    let path = jars_dir.join(format!("{sha}.json"));
    if path.is_file() {
        return None;
    }
    let size = fs::metadata(jar).map(|m| m.len()).unwrap_or(0);
    let report = JarReport {
        schema: SCHEMA,
        install_id: install_id(),
        created_at: now_secs(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        jvm_rev: crate::Runtime::locate()
            .and_then(|rt| rt.manifest().ok())
            .map(|m| m.rev),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        jar_sha256: sha.to_string(),
        jar_size: size,
        family: fp.family.clone(),
        markers: fp.markers.clone(),
        native_libs: fp.native_libs.clone(),
        suffixes: fp.suffixes.clone(),
        needs_native_crypto: fp.needs_native_crypto,
    };
    let json = serde_json::to_string_pretty(&report).ok()?;
    fs::write(&path, json).ok()?;
    Some(path)
}

/// 追加一条失败事件（错误码 + 脱敏详情）；失败不影响主流程。
pub fn note_failure(code: &str, detail: &str) {
    let events = dir().join("events.jsonl");
    if fs::create_dir_all(dir()).is_err() {
        return;
    }
    if fs::metadata(&events).map(|m| m.len() > MAX_EVENTS_BYTES).unwrap_or(false) {
        let _ = fs::rename(&events, dir().join("events.1.jsonl"));
    }
    let event = FailureEvent {
        at: now_secs(),
        code: code.to_string(),
        detail: sanitize(detail, MAX_DETAIL),
    };
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&events) {
        if let Ok(line) = serde_json::to_string(&event) {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// 已 spool 的 jar 报告（按文件名排序，稳定）。
pub fn pending_report_paths() -> Vec<PathBuf> {
    let dir = dir().join("jars");
    let mut paths: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(iter) => iter
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    paths
}

/// 读取全部 jar 报告。
pub fn pending_reports() -> Vec<JarReport> {
    pending_report_paths()
        .into_iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect()
}

/// 读取最近 N 条失败事件（旧→新）。
pub fn recent_failures(limit: usize) -> Vec<FailureEvent> {
    let text = fs::read_to_string(dir().join("events.jsonl")).unwrap_or_default();
    let mut events: Vec<FailureEvent> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if events.len() > limit {
        events.drain(..events.len() - limit);
    }
    events
}

/// 导出诊断包（单个 JSON 文件，用户可自行发给开发者）。
pub fn export_bundle(dest_dir: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dest_dir)?;
    let path = dest_dir.join(format!("vbox-diagnostics-{}.json", now_secs()));
    let bundle = serde_json::json!({
        "schema": SCHEMA,
        "generatedAt": now_secs(),
        "appVersion": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "installId": install_id(),
        "jvmRev": crate::Runtime::locate()
            .and_then(|rt| rt.manifest().ok())
            .map(|m| m.rev),
        "jars": pending_reports(),
        "failures": recent_failures(50),
    });
    let text = serde_json::to_string_pretty(&bundle)
        .map_err(std::io::Error::other)?;
    fs::write(&path, text)?;
    Ok(path)
}

/// 生成「预填好内容的 GitHub Issue」链接（浏览器里打开即成为一条反馈）。
///
/// 仓库可用 `VBOX_TELEMETRY_REPO` 覆盖（默认 `q310550690/VBox`）。
/// 机器可解析的反馈载荷（供 GitHub Action 汇总；同时嵌入 Issue 正文的标记块）。
///
/// 字段约定见 `docs/type3兼容/遥测与兼容性反馈.md`；`schema` 变化时必须同步更新
/// `.github/scripts/telemetry-aggregate.mjs` 的解析逻辑。
pub fn issue_payload() -> serde_json::Value {
    let reports = pending_reports();
    let failures = recent_failures(20);
    let jvm_rev = crate::Runtime::locate()
        .and_then(|rt| rt.manifest().ok())
        .map(|m| m.rev);
    serde_json::json!({
        "schema": SCHEMA,
        "app_version": env!("CARGO_PKG_VERSION"),
        "jvm_rev": jvm_rev,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "install_id": install_id(),
        "jars": reports
            .iter()
            .take(10)
            .map(|r| serde_json::json!({
                "sha256": r.jar_sha256,
                "size": r.jar_size,
                "family": r.family,
                "markers": r.markers,
                "native_libs": r.native_libs,
                "needs_native_crypto": r.needs_native_crypto,
            }))
            .collect::<Vec<_>>(),
        "failures": failures
            .iter()
            .map(|f| serde_json::json!({
                "code": f.code,
                "detail": f.detail.lines().next().unwrap_or(""),
            }))
            .collect::<Vec<_>>(),
    })
}

/// 从 Issue 正文中提取机器可解析块（与 GitHub Action 的解析规则保持一致）。
pub fn extract_payload(body: &str) -> Option<serde_json::Value> {
    const START: &str = "<!-- vbox-telemetry:start -->";
    const END: &str = "<!-- vbox-telemetry:end -->";
    let start = body.find(START)? + START.len();
    let rest = &body[start..];
    let end = rest.find(END)?;
    let json = rest[..end].trim();
    let json = json
        .strip_prefix("```json")
        .map(|s| s.trim_end_matches("```").trim())
        .unwrap_or(json);
    serde_json::from_str(json).ok()
}

pub fn issue_url() -> String {
    let repo = std::env::var("VBOX_TELEMETRY_REPO")
        .ok()
        .filter(|v| v.contains('/'))
        .unwrap_or_else(|| "q310550690/VBox".to_string());
    let reports = pending_reports();
    let failures = recent_failures(10);
    let mut body = String::new();
    body.push_str("### 自动采集的兼容性信息（本机，未上传）\n\n");
    body.push_str(&format!(
        "- 版本：{}（{} / {}）\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    if let Some(rev) = crate::Runtime::locate()
        .and_then(|rt| rt.manifest().ok())
        .map(|m| m.rev)
    {
        body.push_str(&format!("- JVM 构建：`{rev}`\n"));
    }
    body.push_str(&format!("- 已记录 jar：{} 只\n\n", reports.len()));
    for r in reports.iter().take(10) {
        body.push_str(&format!(
            "- `{}…` {} B · 家族 `{}` · markers `{}`{}\n",
            &r.jar_sha256[..r.jar_sha256.len().min(12)],
            r.jar_size,
            r.family,
            r.markers.join(","),
            if r.needs_native_crypto { " · 依赖 native 解密" } else { "" }
        ));
    }
    if !failures.is_empty() {
        body.push_str("\n### 最近的失败码\n\n");
        for f in failures.iter().rev().take(10) {
            body.push_str(&format!("- `{}`：{}\n", f.code, f.detail.lines().next().unwrap_or("")));
        }
    }
    // 机器可解析块：GitHub Action 按 `<!-- vbox-telemetry:start/end -->` 解析并汇总。
    let payload_json =
        serde_json::to_string_pretty(&issue_payload()).unwrap_or_else(|_| "{}".to_string());
    body.push_str(&format!(
        "\n<!-- vbox-telemetry:start -->\n```json\n{payload_json}\n```\n<!-- vbox-telemetry:end -->\n"
    ));
    body.push_str("\n需要我补充的信息：\n");
    let title = url_encode("兼容性反馈：新加固壳 jar / JVM 车道问题");
    let body = url_encode(&body);
    format!(
        "https://github.com/{repo}/issues/new?title={title}&body={body}&labels=shell-family"
    )
}

/// 极简 percent-encode（只保留 unreserved 字符）。
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 远端上报端点的当前配置（未配置 = 关闭）。
pub fn endpoint() -> Option<String> {
    std::env::var("VBOX_TELEMETRY_ENDPOINT")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| v.starts_with("https://"))
}

/// 当前 spool 状态（设置页展示用）。
pub fn status() -> serde_json::Value {
    serde_json::json!({
        "dir": dir().display().to_string(),
        "installId": format!("{}…", &install_id()[..8.min(install_id().len())]),
        "pendingJars": pending_report_paths().len(),
        "pendingFailures": recent_failures(usize::MAX).len(),
        "endpoint": endpoint(),
        "uploadEnabled": false,
    })
}

/// 清空本机 spool（返回释放的字节数）。
pub fn clear() -> u64 {
    let dir = dir();
    let mut freed = 0u64;
    for sub in ["jars", ""] {
        let target = if sub.is_empty() { dir.clone() } else { dir.join(sub) };
        let Ok(entries) = fs::read_dir(&target) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            freed += entry.metadata().map(|m| m.len()).unwrap_or(0);
            let _ = fs::remove_file(&path);
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_query_and_secrets() {
        let s = sanitize(
            "GET https://x.test/a?token=abc123#frag\nAuthorization: Bearer tok\nplain line",
            400,
        );
        assert!(s.contains("https://x.test/a"), "{s}");
        assert!(!s.contains("abc123"), "{s}");
        assert!(!s.contains("tok"), "{s}");
        assert!(!s.contains("#frag"), "{s}");
        assert!(s.contains("plain line"), "{s}");
    }

    #[test]
    fn sanitize_respects_limit() {
        let long = "x".repeat(500);
        let s = sanitize(&long, 40);
        assert!(s.chars().count() <= 41, "{}", s.chars().count());
    }

    #[test]
    fn url_encode_is_percent_encoding() {
        assert_eq!(url_encode("a b&c=d/e?f#g"), "a%20b%26c%3Dd%2Fe%3Ff%23g");
        assert_eq!(url_encode("中文"), "%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn hex16_is_lowercase_hex_prefix() {
        let h = hex16(&[0xde, 0xad, 0xbe, 0xef, 0, 1, 2, 3]);
        assert_eq!(h, "deadbeef00010203");
    }

    #[test]
    fn endpoint_rejects_non_https() {
        std::env::set_var("VBOX_TELEMETRY_ENDPOINT", "ftp://x");
        assert!(endpoint().is_none());
        std::env::remove_var("VBOX_TELEMETRY_ENDPOINT");
    }

    #[test]
    fn export_bundle_writes_single_json() {
        let dest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/telemetry-test");
        let _ = std::fs::remove_dir_all(&dest);
        let path = export_bundle(&dest).expect("export");
        assert!(path.is_file(), "{}", path.display());
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["schema"], 1);
        assert!(v.get("jars").is_some() && v.get("failures").is_some());
    }

    #[test]
    fn issue_body_embeds_machine_readable_payload() {
        let payload = issue_payload();
        assert_eq!(payload["schema"], SCHEMA);
        let json = serde_json::to_string_pretty(&payload).unwrap();
        let body = format!(
            "前言\n\n<!-- vbox-telemetry:start -->\n```json\n{json}\n```\n<!-- vbox-telemetry:end -->\n\n需要补充的信息"
        );
        let parsed = extract_payload(&body).expect("应能解析出机器块");
        assert_eq!(parsed["schema"], SCHEMA);
        assert!(parsed["jars"].is_array());
        assert!(parsed["failures"].is_array());
    }

    #[test]
    fn issue_url_targets_repo_and_encodes_body() {
        std::env::remove_var("VBOX_TELEMETRY_REPO");
        let url = issue_url();
        assert!(url.starts_with("https://github.com/"), "{url}");
        assert!(url.contains("/issues/new?"), "{url}");
        assert!(url.contains("title=") && url.contains("body="), "{url}");
        assert!(!url.contains(' '), "URL 不应含裸空格：{url}");
    }
}
