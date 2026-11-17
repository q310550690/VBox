//! PikPak 网盘客户端（W10.3）。
//!
//! PikPak 与迅雷共用 drive 协议，差异只在 base 与登录：
//! - drive 根 `https://api-drive.mypikpak.com/drive/v1/`（`C0032l0.java:15`）
//! - 账号根 `https://user.mypikpak.com/v1/`（`C0032l0.java:24`）
//! - client_id `YNxT9w7GMdWvEOKa` / client_secret `dbw2OtmVEeuUvIptb1Coyg`（`C0032l0.java:103/108`）
//!
//! FxzHm 版 `O()` 对 PikPak 跳过 captcha 检查（`v0.java:553`），因此登录只需
//! `POST {auth}auth/signin`，无设备头/无应用层签名。
//!
//! 本模块只做「薄封装 + 默认基址」，协议细节见 [`crate::pan::xunlei`]。

use crate::error::Result;
use crate::model::Rules;
use crate::pan::xunlei::{
    LoginTokens, ShareRef, XunleiDrive, PIKPAK_CLIENT_ID, PIKPAK_CLIENT_SECRET,
    PIKPAK_SHARE_WEB,
};
use crate::pan::PanEntry;
use serde_json::Value;

/// PikPak 默认分享域名（`C0032l0.java:25`）。
pub const SHARE_WEB: &str = PIKPAK_SHARE_WEB;

/// PikPak 客户端：drive 基座 + 账号登录。
pub struct PikPakClient {
    drive: XunleiDrive,
}

impl PikPakClient {
    /// `token` 为空串表示匿名（只能浏览公开分享）。
    pub fn new(client: reqwest::blocking::Client, token: &str) -> Self {
        Self {
            drive: XunleiDrive::pikpak(client, token),
        }
    }

    /// 测试用：自定义 drive/账号根（末尾需带 `/`）。
    pub fn with_base(
        client: reqwest::blocking::Client,
        token: &str,
        api_base: &str,
        auth_base: &str,
    ) -> Self {
        Self {
            drive: XunleiDrive::with_bases(
                client,
                token,
                api_base,
                auth_base,
                "PikPak",
                PIKPAK_CLIENT_ID,
                PIKPAK_CLIENT_SECRET,
            ),
        }
    }

    /// 由 Rules 构建默认客户端。
    pub fn with_rules(rules: &Rules, token: &str) -> Result<Self> {
        Ok(Self {
            drive: XunleiDrive::with_rules(rules, token)?,
        })
    }

    pub fn has_token(&self) -> bool {
        self.drive.has_token()
    }

    /// 账号登录并写入登录态；返回可持久化的 token 集。
    pub fn login(&mut self, username: &str, password: &str) -> Result<LoginTokens> {
        let tokens = self.drive.login(username, password)?;
        self.drive.set_token(&tokens.access_token);
        Ok(tokens)
    }

    /// 刷新登录态（`refresh_token` 流程）。
    pub fn refresh(&mut self, refresh_token: &str) -> Result<LoginTokens> {
        let tokens = self.drive.refresh(refresh_token)?;
        self.drive.set_token(&tokens.access_token);
        Ok(tokens)
    }

    /// 分享目录（根或子目录），返回标题 + 条目。
    pub fn share_files(
        &self,
        share_id: &str,
        passcode: &str,
        parent_id: &str,
    ) -> Result<(String, Vec<PanEntry>)> {
        self.drive.share_files(share_id, passcode, parent_id)
    }

    /// 分享文件直链。
    pub fn download_url(
        &self,
        share_id: &str,
        passcode: &str,
        pass_code_token: &str,
        file_id: &str,
    ) -> Result<String> {
        self.drive
            .download_url(share_id, passcode, pass_code_token, file_id)
    }

    /// 分享口令（匿名可用）。
    pub fn share_token(&self, share_id: &str, passcode: &str) -> Result<Value> {
        let token = self.drive.share_token(share_id, passcode)?;
        Ok(serde_json::json!({
            "pass_code_token": token.pass_code_token,
            "title": token.title,
            "share_id": token.share_id,
        }))
    }

    /// 浏览分享链接（`https://mypikpak.com/s/{id}?pwd=xxxx`）。
    pub fn browse(&self, raw: &str) -> Result<(ShareRef, String, Vec<PanEntry>)> {
        let link = crate::pan::xunlei::parse_share(raw).ok_or_else(|| crate::error::CoreError::Config(
            format!("PikPak 分享链接无法解析: {raw}"),
        ))?;
        let (title, files) = self.share_files(&link.share_id, &link.passcode, "")?;
        Ok((link, title, files))
    }
}

/// 判断一个链接是否 PikPak 分享域名。
pub fn is_share_link(raw: &str) -> bool {
    raw.trim().starts_with(SHARE_WEB)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::client_for;

    const LOGIN_JSON: &str =
        r#"{"access_token":"at","refresh_token":"rt","expires_in":7200,"token_type":"Bearer"}"#;
    const SHARE_JSON: &str = r#"{"pass_code_token":"pct","title":"PikPak 分享"}"#;
    const DETAIL_JSON: &str = r#"{"files":[{"id":"f1","name":"a.mp4","kind":"file","category":"video","size":1}],"next_page_token":""}"#;

    fn serve(routes: &'static [(&'static str, &'static str)]) -> String {
        use tiny_http::{Header, Response, Server};
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", server.server_addr());
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let path = req.url().split('?').next().unwrap_or("/").to_string();
                let body = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, b)| *b)
                    .unwrap_or("{}");
                let resp = Response::from_string(body).with_header(
                    Header::from_bytes("Content-Type", "application/json").unwrap(),
                );
                let _ = req.respond(resp);
            }
        });
        addr
    }

    #[test]
    fn login_sets_token_and_browses_share() {
        let base = serve(&[
            ("/auth/signin", LOGIN_JSON),
            ("/share", SHARE_JSON),
            ("/share/detail", DETAIL_JSON),
        ]);
        let client = client_for(&Rules::default()).unwrap();
        let mut c = PikPakClient::with_base(
            client,
            "",
            &format!("{base}/"),
            &format!("{base}/"),
        );
        assert!(!c.has_token());
        let tokens = c.login("u", "p").unwrap();
        assert_eq!(tokens.access_token, "at");
        assert!(c.has_token());
        let (link, title, files) = c
            .browse("https://mypikpak.com/s/xyz987?pwd=1a2b")
            .unwrap();
        assert_eq!(link.share_id, "xyz987");
        assert_eq!(title, "PikPak 分享");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].fid, "f1");
    }

    #[test]
    fn share_link_detection() {
        assert!(is_share_link("https://mypikpak.com/s/abc"));
        assert!(!is_share_link("https://pan.xunlei.com/s/abc"));
    }
}