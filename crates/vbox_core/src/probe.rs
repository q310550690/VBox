//! 直链可用性探测（解析链兜底）：
//! - 先发 `HEAD`；服务器拒绝/失败时回退 `Range: bytes=0-0` 的 GET 校验；
//! - 明确 4xx/5xx 才判「不可用」，网络错误/超时判「无法判断」（避免误杀可用直链）；
//! - `416 Range Not Satisfiable` 视为存在（只是长度为 0/越界）。

use std::collections::HashMap;
use std::time::Duration;

use crate::http;
use crate::model::Rules;

/// 探测结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 可用（2xx/3xx，或 416）。
    Available,
    /// 明确不可用（4xx/5xx / 明显不是媒体）。
    Unavailable(String),
    /// 探测本身失败（网络受限、超时、服务器拒绝探测）→ 不应据此判死。
    Inconclusive(String),
}

/// 探测一个直链是否可访问。`headers` 会透传给 HEAD / Range GET。
pub fn probe_media_url(url: &str, headers: &HashMap<String, String>) -> ProbeOutcome {
    let rules = Rules::default();
    let client = match http::client_for(&rules) {
        Ok(client) => client,
        Err(err) => return ProbeOutcome::Inconclusive(format!("HTTP 客户端创建失败: {err}")),
    };

    let mut head = client.head(url).timeout(Duration::from_secs(8));
    for (key, value) in headers {
        if !key.trim().is_empty() && !value.trim().is_empty() {
            head = head.header(key.trim(), value.trim());
        }
    }
    match head.send() {
        Ok(response) => {
            let status = response.status().as_u16();
            if (200..400).contains(&status) {
                return ProbeOutcome::Available;
            }
            // 服务器拒绝 HEAD（403/405/501）或 404/410 等：回退 Range GET 再判定
            match range_get(&client, url, headers) {
                ProbeOutcome::Available => ProbeOutcome::Available,
                ProbeOutcome::Unavailable(reason) => {
                    ProbeOutcome::Unavailable(format!("HEAD {status}；Range 校验: {reason}"))
                }
                ProbeOutcome::Inconclusive(reason) => {
                    ProbeOutcome::Inconclusive(format!("HEAD {status}；Range 探测失败: {reason}"))
                }
            }
        }
        Err(err) => {
            // 网络层失败可能只是服务器/代理不允许 HEAD：再试 Range GET
            match range_get(&client, url, headers) {
                ProbeOutcome::Available => ProbeOutcome::Available,
                ProbeOutcome::Unavailable(reason) => {
                    ProbeOutcome::Unavailable(format!("HEAD 请求失败: {err}；Range 校验: {reason}"))
                }
                ProbeOutcome::Inconclusive(reason) => {
                    ProbeOutcome::Inconclusive(format!("HEAD 请求失败: {err}；Range: {reason}"))
                }
            }
        }
    }
}

fn range_get(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &HashMap<String, String>,
) -> ProbeOutcome {
    let mut request = client
        .get(url)
        .header("Range", "bytes=0-0")
        .timeout(Duration::from_secs(10));
    for (key, value) in headers {
        if !key.trim().is_empty() && !value.trim().is_empty() {
            request = request.header(key.trim(), value.trim());
        }
    }
    match request.send() {
        Ok(response) => {
            let status = response.status().as_u16();
            if (200..300).contains(&status) || status == 416 {
                ProbeOutcome::Available
            } else {
                ProbeOutcome::Unavailable(format!("HTTP {status}"))
            }
        }
        Err(err) => ProbeOutcome::Inconclusive(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 连不上时不能误判为「不可用」。
    #[test]
    fn probe_unreachable_is_inconclusive() {
        let outcome = probe_media_url("http://127.0.0.1:1/never.mp4", &HashMap::new());
        assert!(
            matches!(outcome, ProbeOutcome::Inconclusive(_)),
            "unreachable outcome = {outcome:?}"
        );
    }

    /// 本地 HTTP 服务器：HEAD 405 时用 Range GET 校验；404 明确不可用。
    #[test]
    fn probe_head_fallback_and_unavailable() {
        use tiny_http::{Response, Server};
        let server = Server::http("127.0.0.1:0").expect("启动测试 HTTP 失败");
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for request in server.incoming_requests().take(4) {
                let path = request.url().to_string();
                if path.starts_with("/missing") {
                    let _ = request.respond(Response::empty(404));
                } else if request.method() == &tiny_http::Method::Head {
                    let _ = request.respond(Response::empty(405));
                } else {
                    let _ = request.respond(Response::from_data(vec![0u8]));
                }
            }
        });

        let ok = probe_media_url(&format!("{base}/ok.mp4"), &HashMap::new());
        assert_eq!(ok, ProbeOutcome::Available, "HEAD 405 后 Range 应成功");
        let missing = probe_media_url(&format!("{base}/missing.mp4"), &HashMap::new());
        assert!(
            matches!(missing, ProbeOutcome::Unavailable(_)),
            "404 应为不可用: {missing:?}"
        );
        let _ = handle.join();
    }
}
