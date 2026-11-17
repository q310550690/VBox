//! 只读云盘管理（W14 P2 / §9）。
//!
//! 目标：在既有 `pan/*` 分享协议之上补一层「个人网盘只读浏览」：
//! 列目录（[`DriveClient::list`]）与取下载直链（[`DriveClient::download_url`]）。
//!
//! 凭据来自 W13 的 [`Credential`]（按 `account_id` 主键存于本机 DPAPI 加密文件）；
//! 本模块只接收内存中的 cookie/token，不落盘、不打日志、不把明文返回前端。
//!
//! 支持的 provider（2026-11-17 依据公开协议实现，未真机验证）：
//! - `p115`：`GET https://webapi.115.com/files?aid=1&cid=<dir>&...`（`cid=0` 为根）
//! - `quark`：`GET https://drive.quark.cn/1/clouddrive/file/sort?pr=ucpro&fr=pc&pdir_fid=<dir>`
//! - `uc`：同夸克，接口根 `https://pc-api.uc.cn/1/clouddrive/`、查询串 `pr=UCBrowser&fr=pc`
//! - `xunlei`：`GET https://api-pan.xunlei.com/drive/v1/files?parent_id=<dir>`
//!
//! 已知限制（W14 P2 范围）：
//! - 115 个人盘下载直链需要 pickcode 加密流程（RSA + `downurl` 接口），暂未接入；
//! - 阿里云盘 / PikPak 的网页登录 Cookie 不含 access token，暂不支持只读浏览；
//! - 列表只取第一页（上限 200/100 条），翻页留待后续按需扩展。

pub mod p115;
pub mod quark;
pub mod xunlei;

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::model::Rules;
use crate::pan::login::{self, Credential};

/// 115 webapi 根。
pub const P115_BASE: &str = "https://webapi.115.com/";
/// 夸克 drive 接口根。
pub const QUARK_BASE: &str = "https://drive.quark.cn/1/clouddrive/";
/// UC drive 接口根。
pub const UC_BASE: &str = "https://pc-api.uc.cn/1/clouddrive/";
/// 迅雷 drive 接口根。
pub const XUNLEI_BASE: &str = "https://api-pan.xunlei.com/drive/v1/";

/// 支持只读浏览的 provider。
pub const BROWSABLE: &[&str] = &["p115", "quark", "uc", "xunlei"];

/// 该 provider 是否支持只读云盘浏览。
pub fn is_browsable(provider: &str) -> bool {
    BROWSABLE.contains(&provider)
}

/// 只读目录项（W14 §9）。
///
/// 字段与前端 `CloudDriveEntry` 一一对应（snake_case 序列化），
/// `id` 在文件场景是文件 id、目录场景是目录 id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DriveEntry {
    /// 文件/目录 id（用于进入子目录或取直链）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 是否目录。
    pub is_dir: bool,
    /// 文件字节数（目录为 `None`）。
    pub size: Option<u64>,
    /// 修改时间（Unix 秒；未知为 `None`）。
    pub modified_at: Option<i64>,
}

impl DriveEntry {
    /// 构造一个最小条目。
    pub fn new(id: impl Into<String>, name: impl Into<String>, is_dir: bool) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            is_dir,
            size: None,
            modified_at: None,
        }
    }
}

/// 只读云盘抽象（W14 §9）。
///
/// 入参 `account_id` 由调用方（Tauri 命令）用凭据存储解析成 [`DriveClient`] 后不再需要；
/// 需要多账号同时浏览时按账号各建一个客户端即可。
pub trait DriveRead {
    /// 列出 `parent_id` 目录（`None`/空串 = 根目录）。
    fn list(&self, parent_id: Option<&str>) -> Result<Vec<DriveEntry>>;
    /// 取文件下载直链。
    fn download_url(&self, entry_id: &str) -> Result<String>;
}

/// 已绑定凭据的只读云盘客户端。
pub struct DriveClient {
    provider: String,
    client: reqwest::blocking::Client,
    base: String,
    cookie: Option<String>,
    token: Option<String>,
}

impl DriveClient {
    /// 从 W13 凭据构造（provider 归一化 + 凭据按 provider 校验 + 默认接口根）。
    pub fn from_credential(cred: &Credential) -> Result<Self> {
        let provider = login::normalize_provider(&cred.provider)
            .ok_or_else(|| CoreError::Config(format!("未知网盘 provider：{}", cred.provider)))?;
        let base = default_base(&provider)
            .ok_or_else(|| unsupported(&provider))?
            .to_string();
        let client = crate::http::client_for(&Rules::default())?;
        let this = Self {
            provider,
            client,
            base,
            cookie: cred.cookie.clone().filter(|s| !s.trim().is_empty()),
            token: cred.token.clone().filter(|s| !s.trim().is_empty()),
        };
        this.ensure_ready()?;
        Ok(this)
    }

    /// 自定义客户端/接口根（供 mock 测试使用）。
    pub fn with_parts(
        provider: &str,
        client: reqwest::blocking::Client,
        base: &str,
        cookie: Option<String>,
        token: Option<String>,
    ) -> Result<Self> {
        let provider = login::normalize_provider(provider)
            .ok_or_else(|| CoreError::Config(format!("未知网盘 provider：{provider}")))?;
        if default_base(&provider).is_none() {
            return Err(unsupported(&provider));
        }
        let this = Self {
            provider,
            client,
            base: base.to_string(),
            cookie: cookie.filter(|s| !s.trim().is_empty()),
            token: token.filter(|s| !s.trim().is_empty()),
        };
        this.ensure_ready()?;
        Ok(this)
    }

    /// provider 标识（`p115` / `quark` / `uc` / `xunlei`）。
    pub fn provider(&self) -> &str {
        &self.provider
    }

    fn cookie(&self) -> &str {
        self.cookie.as_deref().unwrap_or("")
    }

    fn token(&self) -> &str {
        self.token.as_deref().unwrap_or("")
    }

    /// 凭据是否满足该 provider 的最小要求（cookie 或 token）。
    fn ensure_ready(&self) -> Result<()> {
        match self.provider.as_str() {
            "p115" | "quark" | "uc" => {
                if self.cookie.is_none() {
                    return Err(CoreError::Config(format!(
                        "{} 需要登录 Cookie：请在云盘首页右上角「账号管理」重新登录",
                        display_name(&self.provider)
                    )));
                }
            }
            "xunlei" => {
                if self.token.is_none() {
                    return Err(CoreError::Config(
                        "迅雷需要登录 token：请在云盘首页右上角「账号管理」重新登录".into(),
                    ));
                }
            }
            other => return Err(unsupported(other)),
        }
        Ok(())
    }
}

impl DriveRead for DriveClient {
    fn list(&self, parent_id: Option<&str>) -> Result<Vec<DriveEntry>> {
        match self.provider.as_str() {
            "p115" => p115::list(&self.client, &self.base, self.cookie(), parent_id),
            "quark" => quark::list(
                &self.client,
                &self.base,
                crate::pan::quark::QUERY,
                "夸克",
                self.cookie(),
                parent_id,
            ),
            "uc" => quark::list(
                &self.client,
                &self.base,
                crate::pan::uc::QUERY,
                "UC",
                self.cookie(),
                parent_id,
            ),
            "xunlei" => xunlei::list(&self.client, &self.base, self.token(), parent_id),
            other => Err(unsupported(other)),
        }
    }

    fn download_url(&self, entry_id: &str) -> Result<String> {
        match self.provider.as_str() {
            "quark" => quark::download_url(
                &self.client,
                &self.base,
                crate::pan::quark::QUERY,
                "夸克",
                self.cookie(),
                entry_id,
            ),
            "uc" => quark::download_url(
                &self.client,
                &self.base,
                crate::pan::uc::QUERY,
                "UC",
                self.cookie(),
                entry_id,
            ),
            "xunlei" => xunlei::download_url(&self.client, &self.base, self.token(), entry_id),
            "p115" => Err(CoreError::Config(
                "115 个人盘下载直链需要 pickcode 加密流程，W14 P2 暂未接入".into(),
            )),
            other => Err(unsupported(other)),
        }
    }
}

fn default_base(provider: &str) -> Option<&'static str> {
    match provider {
        "p115" => Some(P115_BASE),
        "quark" => Some(QUARK_BASE),
        "uc" => Some(UC_BASE),
        "xunlei" => Some(XUNLEI_BASE),
        _ => None,
    }
}

fn display_name(provider: &str) -> &'static str {
    match provider {
        "p115" => "115",
        "quark" => "夸克",
        "uc" => "UC",
        "xunlei" => "迅雷",
        "ali" => "阿里",
        "pikpak" => "PikPak",
        _ => "该网盘",
    }
}

fn unsupported(provider: &str) -> CoreError {
    CoreError::Config(format!(
        "{} 暂不支持只读云盘浏览（W14 P2 后续接入）",
        display_name(provider)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tiny_http::Request;

    /// 起一个按 `handler(&request)` 返回 JSON 的本地 mock 服务。
    fn spawn_json_server<F>(handler: F) -> (String, std::thread::JoinHandle<()>)
    where
        F: Fn(&Request) -> String + Send + 'static,
    {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = handler(&req);
                let resp = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        (base, handle)
    }

    fn test_client() -> reqwest::blocking::Client {
        crate::http::client_for(&Rules::default()).unwrap()
    }

    fn header_of(req: &Request, name: &str) -> Option<String> {
        req.headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str().to_string())
    }

    #[test]
    fn p115_list_reads_personal_dir() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        let (base, _h) = spawn_json_server(move |req| {
            seen2.lock().unwrap().push(req.url().to_string());
            assert!(header_of(req, "cookie")
                .unwrap_or_default()
                .contains("UID=u1"));
            r#"{"state":true,"data":[
                {"fid":"f1","cid":"0","n":"第1集.mp4","s":1024,"te":1700000000},
                {"cid":"d1","n":"合集","t":1}
            ]}"#
            .to_string()
        });
        let client = DriveClient::with_parts(
            "p115",
            test_client(),
            &base,
            Some("UID=u1; CID=c1".into()),
            None,
        )
        .unwrap();
        let entries = client.list(None).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "f1");
        assert!(!entries[0].is_dir);
        assert!(entries[1].is_dir);
        assert!(seen.lock().unwrap()[0].contains("cid=0"));

        let entries = client.list(Some("d1")).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(seen.lock().unwrap()[1].contains("cid=d1"));
    }

    #[test]
    fn quark_list_and_download_over_http() {
        let seen = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let seen2 = seen.clone();
        let (base, _h) = spawn_json_server(move |req| {
            let url = req.url().to_string();
            let cookie = header_of(req, "cookie").unwrap_or_default();
            seen2.lock().unwrap().push((url.clone(), cookie));
            if url.starts_with("/file/download") {
                r#"{"status":200,"code":0,"message":"ok","data":[{"download_url":"https://dl/quark/1"}]}"#
                    .to_string()
            } else {
                r#"{"status":200,"code":0,"message":"ok","data":{"list":[
                    {"fid":"f1","file_name":"第1集.mp4","dir":false,"size":2048,"updated_at":1700000000},
                    {"fid":"d1","file_name":"合集","dir":true,"size":0}
                ]}}"#
                .to_string()
            }
        });
        let client = DriveClient::with_parts(
            "quark",
            test_client(),
            &base,
            Some("__pus=x; __puus=y".into()),
            None,
        )
        .unwrap();
        let entries = client.list(None).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].size, Some(2048));
        assert!(entries[1].is_dir);
        let url = client.download_url("f1").unwrap();
        assert_eq!(url, "https://dl/quark/1");
        let calls = seen.lock().unwrap();
        assert!(calls[0].0.contains("pdir_fid=0"));
        assert!(calls[0].1.contains("__puus=y"));
        assert!(calls[1].0.contains("file/download"));
    }

    #[test]
    fn xunlei_list_sends_bearer_and_download_reads_links() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        let (base, _h) = spawn_json_server(move |req| {
            seen2.lock().unwrap().push(format!(
                "{} {}",
                req.url(),
                header_of(req, "authorization").unwrap_or_default()
            ));
            if req.url().starts_with("/files/f1") {
                r#"{"file":{"id":"f1","links":{"application/octet-stream":{"url":"https://dl/xl/1"}}}}"#
                    .to_string()
            } else {
                r#"{"files":[
                    {"id":"f1","name":"第1集.mp4","kind":"file","size":2048},
                    {"id":"d1","name":"合集","kind":"folder","size":0}
                ]}"#
                .to_string()
            }
        });
        let client =
            DriveClient::with_parts("xunlei", test_client(), &base, None, Some("token-1".into()))
                .unwrap();
        let entries = client.list(Some("d1")).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].is_dir);
        assert_eq!(client.download_url("f1").unwrap(), "https://dl/xl/1");
        let calls = seen.lock().unwrap();
        assert!(calls[0].contains("parent_id=d1"));
        assert!(calls[0].contains("Bearer token-1"));
    }

    #[test]
    fn unsupported_provider_and_missing_credential_are_config_errors() {
        let client = test_client();
        let ali = DriveClient::with_parts(
            "ali",
            client.clone(),
            "http://127.0.0.1:1/",
            Some("c".into()),
            None,
        )
        .err()
        .expect("ali 应不支持");
        assert!(matches!(ali, CoreError::Config(msg) if msg.contains("阿里")));

        let no_cookie =
            DriveClient::with_parts("quark", client.clone(), "http://127.0.0.1:1/", None, None)
                .err()
                .expect("缺 cookie 应报错");
        assert!(matches!(no_cookie, CoreError::Config(msg) if msg.contains("重新登录")));

        let no_token = DriveClient::with_parts("xunlei", client, "http://127.0.0.1:1/", None, None)
            .err()
            .expect("缺 token 应报错");
        assert!(matches!(no_token, CoreError::Config(msg) if msg.contains("token")));
    }

    #[test]
    fn p115_download_is_explicitly_unsupported() {
        let (base, _h) = spawn_json_server(|_| r#"{"state":true,"data":[]}"#.to_string());
        let client =
            DriveClient::with_parts("p115", test_client(), &base, Some("UID=u1".into()), None)
                .unwrap();
        assert!(client.list(None).unwrap().is_empty());
        let err = client.download_url("f1").unwrap_err();
        assert!(matches!(err, CoreError::Config(msg) if msg.contains("pickcode")));
    }

    #[test]
    fn from_credential_uses_default_base_and_validates() {
        let mut cred = Credential::new("quark");
        cred.cookie = Some("__pus=x".into());
        let client = DriveClient::from_credential(&cred).unwrap();
        assert_eq!(client.provider(), "quark");

        let mut cred = Credential::new("ali");
        cred.cookie = Some("c".into());
        assert!(DriveClient::from_credential(&cred).is_err());

        let mut cred = Credential::new("xunlei");
        cred.token = Some("t".into());
        assert_eq!(
            DriveClient::from_credential(&cred).unwrap().provider(),
            "xunlei"
        );

        let cred = Credential::new("");
        assert!(DriveClient::from_credential(&cred).is_err());
    }
}
