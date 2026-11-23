use serde::{Deserialize, Serialize};

/// `--serve` 模式的行信封（与 `jvm/host/src/SpiderHost.java` 的 writeEnvelope 对齐）。
///
/// ```text
/// {"id":"7","ok":true,"data":"{...}","pp":45231}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// 请求 id；一次性模式下宿主不回填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub ok: bool,
    /// 蜘蛛返回的 JSON 文本（字符串，宿主不做二次解析）。
    #[serde(default)]
    pub data: String,
    /// 宿主代理端口（`-Dtvbox.proxy.port` 生效时才有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pp: Option<u16>,
    /// 失败原因（宿主的稳定错误码 + 明细，见计划附录 B）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<ProtoErr>,
}

/// 宿主失败信封里的 `err` 对象。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtoErr {
    /// `class_not_found` / `init_failed` / `method_missing` / `bad_args`
    /// / `spider_threw` / `crashed` / `proxy_bind` …（计划附录 B）。
    pub code: String,
    #[serde(default)]
    pub detail: String,
}

impl ProtoErr {
    pub fn detail_or_empty(&self) -> &str {
        &self.detail
    }
}

impl Envelope {
    /// 把 `data` 当作 JSON 解析（蜘蛛的返回值一定是 JSON 文本）。
    pub fn data_json(&self) -> std::result::Result<serde_json::Value, serde_json::Error> {
        if self.data.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&self.data)
    }
}

/// `SpiderHost --probe` 的单行 JSON 输出。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Probe {
    pub ok: bool,
    #[serde(default)]
    pub java: String,
    #[serde(default)]
    pub vm: String,
    #[serde(default)]
    pub os: String,
    /// 宿主自身 class 文件的 major 版本；必须是 61（Java 17）。
    #[serde(rename = "classVersion", default)]
    pub class_version: i32,
    #[serde(default)]
    pub pid: u64,
    /// `android.content.Context` 是否可见（stubs.jar 在 classpath 上）。
    #[serde(default)]
    pub stubs: bool,
    #[serde(default)]
    pub libs: std::collections::BTreeMap<String, bool>,
    #[serde(rename = "proxyPort", default)]
    pub proxy_port: i32,
    #[serde(rename = "hostJar", default)]
    pub host_jar: String,
    /// 加固壳 ARM 桥是否可用（`native-bridge.jar` + unidbg 在 classpath 上）。
    #[serde(default)]
    pub bridge: bool,
    /// unidbg 模拟器类是否可见（缺 jar 时 bridge 也不会可用）。
    #[serde(default)]
    pub unidbg: bool,
}

impl Probe {
    /// 装配是否完整：JRE 17、stubs、以及 gson/okhttp/json/sqlite 这些蜘蛛必用库。
    pub fn is_healthy(&self) -> bool {
        self.ok
            && self.class_version == 61
            && self.stubs
            && ["gson", "okhttp", "json", "sqlite"]
                .iter()
                .all(|k| self.libs.get(*k).copied().unwrap_or(false))
    }

    /// 缺项列表（给 CLI/日志用；空 = 健康）。
    pub fn missing(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.ok {
            out.push("probe.ok=false".into());
        }
        if self.class_version != 61 {
            out.push(format!("classVersion={} (期望 61)", self.class_version));
        }
        if !self.stubs {
            out.push("stubs.jar 不在 classpath".into());
        }
        for key in ["gson", "okhttp", "jsoup", "json", "sqlite", "bc"] {
            if !self.libs.get(key).copied().unwrap_or(false) {
                out.push(format!("libs/{key} 缺失"));
            }
        }
        out
    }
}

/// 从一行 stdout 里解析信封；不是 JSON 的行（蜘蛛日志泄漏）返回 `None`。
pub fn parse_envelope(line: &str) -> Option<Envelope> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// 从一行 stdout 里解析探针结果。
pub fn parse_probe(line: &str) -> Option<Probe> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_with_pp() {
        let env = parse_envelope(r#"{"id":"7","ok":true,"data":"{\"a\":1}","pp":45231}"#).unwrap();
        assert_eq!(env.id.as_deref(), Some("7"));
        assert!(env.ok);
        assert_eq!(env.pp, Some(45231));
        assert_eq!(env.data_json().unwrap()["a"], 1);
    }

    #[test]
    fn envelope_tolerates_log_noise() {
        assert!(parse_envelope("I/SpiderLog: 开始搜索").is_none());
        assert!(parse_envelope("").is_none());
        assert!(parse_envelope(r#"{"ok":true,"data":""}"#).is_some());
    }

    #[test]
    fn probe_health() {
        let p: Probe = serde_json::from_str(
            r#"{"ok":true,"java":"17.0.21","vm":"OpenJDK","os":"amd64/Windows 11","classVersion":61,
                "pid":42,"stubs":true,"libs":{"gson":true,"okhttp":true,"jsoup":true,"json":true,
                "sqlite":true,"bc":true},"proxyPort":-1,"hostJar":"x"}"#,
        )
        .unwrap();
        assert!(p.is_healthy());
        assert!(p.missing().is_empty());
    }
}
