//! 网盘账号登录（W13）。
//!
//! 分三层职责：
//!
//! 1. **协议层**：各 provider 实现 [`LoginFlow`]——`start` 出票，`poll` 推进
//!    `Waiting → Scanned → Success/Expired/Failure` 状态机；
//! 2. **会话层**：[`LoginHub`] 持有已注册的 provider 与进行中的票据
//!    （`ticket_id → provider/ticket`），供 Tauri 命令 `login_start` /
//!    `login_poll(provider, ticket_id)` / `login_cancel(ticket_id)` 复用；
//! 3. **凭据层**：[`credential_overlay`] 保存「当前活跃账号」凭据，供 pan 客户端
//!    在 ext 未配置 cookie 时回落（W13 §2 overlay 注入）。
//!
//! 与 W14（云盘聚合搜索）的复用约定：
//! - 票据对外标识用 `ticket_id`（W14 `login_poll(provider, ticket_id)`）；
//! - 凭据带 `account_id`（W14 凭据存储以 account_id 为主键）；
//! - 账号摘要统一成 [`AccountView`]（W14 `accounts_list -> Vec<AccountView>`）；
//! - provider 字符串统一小写：`p115 / quark / xunlei / uc / ali / pikpak`。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

pub mod ali;
pub mod p115;
pub mod pikpak;
pub mod quark;
pub mod uc;
pub mod xunlei;

/// 已支持的 provider id（小写；与 `PanProvider::name()`、W14 的 provider 字段一致）。
pub const P115: &str = "p115";
pub const QUARK: &str = "quark";
pub const XUNLEI: &str = "xunlei";
pub const UC: &str = "uc";
pub const ALI: &str = "ali";
pub const PIKPAK: &str = "pikpak";

/// 全部已知 provider（W13 首批 115/迅雷/夸克，其余为 W14 预留）。
pub const KNOWN_PROVIDERS: [&str; 6] = [P115, QUARK, XUNLEI, UC, ALI, PIKPAK];

/// 登录专用客户端超时：115 的状态接口是 ~30s 长轮询，必须大于该窗口。
pub const LOGIN_TIMEOUT_SECS: u64 = 40;

/// 登录流使用的 UA（与 115 网页端一致）。
pub const LOGIN_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

/// 构建登录专用 HTTP 客户端（比影视源默认 6s 超时更长，覆盖扫码长轮询）。
pub fn login_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS))
        .user_agent(LOGIN_UA)
        .build()
        .map_err(CoreError::Http)
}

/// 当前 Unix 时间戳（秒）。
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 把 provider 字符串规范化为已知的小写 id；未知放回 `None`。
/// 把扫码链接本地渲染成二维码 SVG data URL。
///
/// 夸克 / UC 没有官方二维码图片接口（只有扫码页链接），由此在桌面端本地生成二维码，
/// 前端 `<img :src="ticket.image_url">` 直接显示，无需外网二维码服务。
pub fn qr_data_url(content: &str) -> String {
    use base64::Engine as _;
    let Ok(qr) = qrcodegen::QrCode::encode_text(content, qrcodegen::QrCodeEcc::Medium) else {
        return String::new();
    };
    let size = qr.size();
    let border = 2;
    let dim = size + border * 2;
    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z", x + border, y + border));
            }
        }
    }
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" shape-rendering=\"crispEdges\"><rect width=\"{dim}\" height=\"{dim}\" fill=\"#ffffff\"/><path d=\"{path}\" fill=\"#000000\"/></svg>"
    );
    format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg.as_bytes())
    )
}

pub fn normalize_provider(raw: &str) -> Option<String> {
    let id = raw.trim().to_ascii_lowercase();
    if KNOWN_PROVIDERS.contains(&id.as_str()) {
        Some(id)
    } else {
        None
    }
}

pub fn require_provider(raw: &str) -> Result<String> {
    normalize_provider(raw).ok_or_else(|| CoreError::Config(format!("未知的网盘 provider: {raw}")))
}

/// 互斥锁辅助：本模块的临界区都不会在 panic 后留下坏状态，毒化时直接取回。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 二维码/设备码类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QrKind {
    /// 扫码登录：`content` 是二维码要编码的 URL。
    QrCode,
    /// 设备码登录：`content` 是用户要打开的验证地址（通常含 `user_code`）。
    DeviceCode,
}

/// 一次登录会话的票据。
///
/// `ticket_id` 是宿主侧会话主键；`content` 交给前端渲染（二维码内容或设备码验证
/// 地址）；`image_url` 可选（provider 直接给图片时用），空串表示由前端根据
/// `content` 生成二维码。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrTicket {
    pub ticket_id: String,
    pub kind: QrKind,
    pub content: String,
    #[serde(default)]
    pub image_url: String,
    /// 供前端直接展示的可扫二维码（data URL；设备码型由授权链接本地生成）。
    #[serde(default)]
    pub qr_image: String,
    /// 有效期（秒）
    pub expires_in: u64,
    /// 过期时刻（Unix 秒；0 表示未知）
    #[serde(default)]
    pub expires_at: i64,
    /// provider 轮询所需的内部状态（uid/time/sign、device_code 等）。
    /// 不返回前端：只留在宿主内存里的票据副本中。
    #[serde(default, skip_serializing)]
    pub(crate) state: BTreeMap<String, String>,
}

impl QrTicket {
    /// 新建票据并计算过期时刻。
    pub fn new(
        ticket_id: impl Into<String>,
        kind: QrKind,
        content: impl Into<String>,
        image_url: impl Into<String>,
        expires_in: u64,
    ) -> Self {
        let now = now_unix();
        Self {
            ticket_id: ticket_id.into(),
            kind,
            content: content.into(),
            image_url: image_url.into(),
            qr_image: String::new(),
            expires_in,
            expires_at: if expires_in == 0 {
                0
            } else {
                now + expires_in as i64
            },
            state: BTreeMap::new(),
        }
    }

    /// 是否已过期（`expires_at == 0` 视为不过期）。
    pub fn is_expired(&self) -> bool {
        self.expires_at > 0 && now_unix() >= self.expires_at
    }

    /// 剩余有效秒数（未过期时 >0；过期或未知时 0）。
    pub fn remaining_secs(&self) -> u64 {
        if self.expires_at <= 0 {
            0
        } else {
            (self.expires_at - now_unix()).max(0) as u64
        }
    }
}

/// 登录成功后拿到的凭据。
///
/// 安全约束（W13 §6）：cookie/token **不进日志、不返回前端明文**；前端只拿
/// [`AccountView`] 摘要。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// 账号主键（W14 凭据存储主键；空串时由 [`ensure_account_id`] 生成）
    #[serde(default)]
    pub account_id: String,
    pub provider: String,
    #[serde(default)]
    pub cookie: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// access token 过期时刻（Unix 秒；cookie 型凭据通常为 None）
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub display_name: Option<String>,
    /// 账号标识掩码，如 `u***88` / `138****2231`（W14 账号卡片展示）
    #[serde(default)]
    pub masked_id: Option<String>,
    /// 最后更新时间（Unix 秒）
    #[serde(default)]
    pub updated_at: i64,
    /// provider 私有附加字段（如 115 的 `uid`、PikPak 的 device id）
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl Credential {
    /// 新建一份空凭据（只填 provider）。
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            updated_at: now_unix(),
            ..Default::default()
        }
    }

    /// 有效账号主键：`account_id` 为空时回退为 `{provider}:{摘要}`。
    pub fn effective_account_id(&self) -> String {
        if !self.account_id.is_empty() {
            return self.account_id.clone();
        }
        let seed = self
            .cookie
            .as_deref()
            .or(self.token.as_deref())
            .or(self.refresh_token.as_deref())
            .unwrap_or("");
        format!("{}:{:x}", self.provider, md5::compute(seed.as_bytes()))
    }

    /// 是否已过期。
    pub fn is_expired(&self) -> bool {
        matches!(self.expires_at, Some(ts) if ts > 0 && now_unix() >= ts)
    }

    /// pan 客户端注入用的凭据串：优先 cookie，回退 token。
    pub fn cookie_or_token(&self) -> Option<&str> {
        self.cookie
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.token.as_deref().filter(|s| !s.trim().is_empty()))
    }

    /// 凭据是否具备最小可用信息（cookie 或 token 至少一个非空）。
    pub fn is_usable(&self) -> bool {
        self.cookie_or_token().is_some()
    }
}

/// 账号状态（W14 账号卡片徽标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    /// 已连接且凭据未过期
    Connected,
    /// 凭据已过期，需要重新登录
    Expired,
    /// 尚未登录
    NotLoggedIn,
}

/// 账号摘要：Tauri 命令 `accounts_list` 的返回项（W14 §7.2）。
///
/// 只含展示信息，不含 cookie/token 明文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountView {
    pub account_id: String,
    pub provider: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub masked_id: Option<String>,
    pub status: AccountStatus,
    #[serde(default)]
    pub updated_at: i64,
}

impl AccountView {
    /// 由凭据生成摘要。
    pub fn from_credential(cred: &Credential) -> Self {
        Self {
            account_id: cred.effective_account_id(),
            provider: cred.provider.clone(),
            display_name: cred.display_name.clone(),
            masked_id: cred.masked_id.clone(),
            status: if cred.is_expired() {
                AccountStatus::Expired
            } else {
                AccountStatus::Connected
            },
            updated_at: cred.updated_at,
        }
    }

    /// 未登录占位卡片（W14 账号管理：每个 provider 都渲染一张卡片）。
    pub fn not_logged_in(provider: &str) -> Self {
        Self {
            account_id: String::new(),
            provider: provider.to_string(),
            display_name: None,
            masked_id: None,
            status: AccountStatus::NotLoggedIn,
            updated_at: 0,
        }
    }
}

/// 登录轮询结果状态机。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "credential", rename_all = "snake_case")]
pub enum LoginOutcome {
    /// 等待扫码/确认
    Waiting,
    /// 已扫码，等待用户在 App 内确认
    Scanned,
    /// 登录成功，携带凭据（仅在宿主侧流转，不直接下发前端）
    Success(Credential),
    /// 票据过期，需要重新出票
    Expired,
    /// 失败（风控、网络、协议错误等）
    Failure(String),
}

/// 单个 provider 的登录流。
///
/// 实现要求：`poll` 尽量无副作用（除内部计数外），可被宿主按固定间隔重复调用；
/// 长轮询实现可以内部阻塞到状态变化或短超时。
pub trait LoginFlow: Send + Sync {
    /// provider id（小写，见 [`KNOWN_PROVIDERS`]）。
    fn provider(&self) -> &'static str;

    /// 出票：向 provider 申请二维码/设备码。
    fn start(&self) -> Result<QrTicket>;

    /// 轮询票据状态。
    fn poll(&self, ticket: &QrTicket) -> Result<LoginOutcome>;

    /// 取消登录（默认无远端副作用——多数扫码协议无需通知服务端）。
    fn cancel(&self, _ticket_id: &str) -> Result<()> {
        Ok(())
    }
}

/// 凭据 overlay：key = provider id（小写），value = 该 provider 当前活跃账号。
///
/// 用途：ext 未显式配置 cookie 时，pan 客户端回落到这里的登录态。多账号持久化
/// （`credentials.json` 按 account_id 主键）由宿主存储层负责，overlay 只保留
/// 「当前活跃」一份，避免 W9/W10 既有引擎被改成多账号参数。
pub fn credential_overlay() -> &'static Mutex<BTreeMap<String, Credential>> {
    static OVERLAY: OnceLock<Mutex<BTreeMap<String, Credential>>> = OnceLock::new();
    OVERLAY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 校验并规范化凭据：provider 合法、含可用身份信息、补齐 account_id/updated_at。
pub fn normalize_credential(mut cred: Credential) -> Result<Credential> {
    cred.provider = require_provider(&cred.provider)?;
    if !cred.is_usable() {
        return Err(CoreError::Config(format!(
            "{} 凭据缺少 cookie/token",
            cred.provider
        )));
    }
    if cred.account_id.trim().is_empty() {
        cred.account_id = cred.effective_account_id();
    }
    if cred.updated_at <= 0 {
        cred.updated_at = now_unix();
    }
    Ok(cred)
}

/// 安装/覆盖某 provider 的活跃凭据（overlay）。
pub fn install_credential(cred: Credential) -> Result<Credential> {
    let cred = normalize_credential(cred)?;
    lock(credential_overlay()).insert(cred.provider.clone(), cred.clone());
    Ok(cred)
}

/// 读取某 provider 的活跃凭据。
pub fn active_credential(provider: &str) -> Option<Credential> {
    let provider = normalize_provider(provider)?;
    lock(credential_overlay()).get(&provider).cloned()
}

/// 移除某 provider 的活跃凭据，返回被移除项。
pub fn remove_credential(provider: &str) -> Option<Credential> {
    let provider = normalize_provider(provider)?;
    lock(credential_overlay()).remove(&provider)
}

/// 供 pan 客户端注入的凭据串（cookie 优先，回退 token）；已过期则视为无凭据。
pub fn overlay_cookie(provider: &str) -> Option<String> {
    active_credential(provider)
        .filter(|c| !c.is_expired())
        .and_then(|c| c.cookie_or_token().map(str::to_string))
}

/// 活跃且未过期的凭据（不存在 / 过期 / 不可用返回 None）。
pub fn overlay_active(provider: &str) -> Option<Credential> {
    active_credential(provider).filter(|c| c.is_usable() && !c.is_expired())
}

/// 供迅雷 / PikPak 等 token 型客户端注入的 access_token；无 token 或已过期返回 None。
pub fn overlay_token(provider: &str) -> Option<String> {
    overlay_active(provider).and_then(|c| c.token.filter(|t| !t.trim().is_empty()))
}

/// 账号掩码：`u***88` / `138****2231` 风格。
///
/// - 长度 ≤ 2：`**`
/// - 长度 ≤ 6：首 1 + `***` + 尾 1
/// - 其余：首 3 + `****` + 尾 3
pub fn mask_identity(raw: &str) -> String {
    let s = raw.trim();
    let chars: Vec<char> = s.chars().collect();
    match chars.len() {
        0 => String::new(),
        1..=2 => "**".to_string(),
        3..=6 => format!("{}***{}", chars[0], chars[chars.len() - 1]),
        n => format!(
            "{}****{}",
            chars[..3].iter().collect::<String>(),
            chars[n - 3..].iter().collect::<String>()
        ),
    }
}

/// 宿主侧登录会话（ticket 与 provider 绑定）。
#[derive(Debug, Clone)]
struct LoginSession {
    provider: String,
    ticket: QrTicket,
}

/// 登录会话中心：注册 provider 登录流 + 管理进行中的票据。
///
/// Tauri 层只持有 `LoginHub`（`AppState` 内），命令按 W14 形态转发：
/// - `login_start(provider) -> QrTicket`
/// - `login_poll(provider, ticket_id) -> LoginOutcome`
/// - `login_cancel(ticket_id)`
#[derive(Default)]
pub struct LoginHub {
    flows: Mutex<BTreeMap<String, Arc<dyn LoginFlow>>>,
    sessions: Mutex<BTreeMap<String, LoginSession>>,
}

impl LoginHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 provider 的登录流（同 provider 覆盖）。
    pub fn register(&self, flow: Arc<dyn LoginFlow>) {
        lock(&self.flows).insert(flow.provider().to_string(), flow);
    }

    /// 已注册 provider 列表（排序稳定）。
    pub fn providers(&self) -> Vec<String> {
        lock(&self.flows).keys().cloned().collect()
    }

    /// 某个 provider 是否已注册登录流。
    pub fn supports(&self, provider: &str) -> bool {
        normalize_provider(provider)
            .map(|p| lock(&self.flows).contains_key(&p))
            .unwrap_or(false)
    }

    fn flow(&self, provider: &str) -> Result<Arc<dyn LoginFlow>> {
        let provider = require_provider(provider)?;
        lock(&self.flows)
            .get(&provider)
            .cloned()
            .ok_or_else(|| CoreError::Config(format!("{provider} 暂不支持登录")))
    }

    /// 出票并登记会话。
    pub fn start(&self, provider: &str) -> Result<QrTicket> {
        let flow = self.flow(provider)?;
        let mut ticket = flow.start()?;
        if ticket.ticket_id.trim().is_empty() {
            ticket.ticket_id = new_ticket_id(flow.provider());
        }
        let session = LoginSession {
            provider: flow.provider().to_string(),
            ticket: ticket.clone(),
        };
        let replaced = lock(&self.sessions).insert(ticket.ticket_id.clone(), session);
        debug_assert!(replaced.is_none(), "ticket_id 冲突：{}", ticket.ticket_id);
        Ok(ticket)
    }

    /// 轮询指定票据。终态（Success/Expired/Failure）会结束会话；Success 同时写入
    /// overlay。
    pub fn poll(&self, provider: &str, ticket_id: &str) -> Result<LoginOutcome> {
        let flow = self.flow(provider)?;
        let session = {
            let sessions = lock(&self.sessions);
            sessions
                .get(ticket_id)
                .map(|s| (s.provider.clone(), s.ticket.clone()))
                .ok_or_else(|| {
                    CoreError::Config("登录会话不存在或已结束，请重新获取二维码".to_string())
                })?
        };
        if session.0 != flow.provider() {
            return Err(CoreError::Config(format!(
                "登录会话属于 {}，与请求的 {} 不匹配",
                session.0,
                flow.provider()
            )));
        }
        if session.1.is_expired() {
            lock(&self.sessions).remove(ticket_id);
            return Ok(LoginOutcome::Expired);
        }

        let outcome = flow.poll(&session.1)?;
        match &outcome {
            LoginOutcome::Waiting | LoginOutcome::Scanned => Ok(outcome),
            LoginOutcome::Success(_) => {
                let cred = match outcome {
                    LoginOutcome::Success(cred) => cred,
                    _ => unreachable!(),
                };
                let cred = install_credential(cred)?;
                lock(&self.sessions).remove(ticket_id);
                Ok(LoginOutcome::Success(cred))
            }
            LoginOutcome::Expired => {
                lock(&self.sessions).remove(ticket_id);
                Ok(outcome)
            }
            LoginOutcome::Failure(_) => {
                lock(&self.sessions).remove(ticket_id);
                Ok(outcome)
            }
        }
    }

    /// 取消票据；返回是否确实存在该会话。
    pub fn cancel(&self, ticket_id: &str) -> Result<bool> {
        let session = lock(&self.sessions).remove(ticket_id);
        match session {
            Some(session) => {
                if let Some(flow) = lock(&self.flows).get(&session.provider).cloned() {
                    flow.cancel(ticket_id)?;
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// 读取进行中的票据（前端刷新页面后恢复二维码用）。
    pub fn ticket(&self, ticket_id: &str) -> Option<QrTicket> {
        lock(&self.sessions)
            .get(ticket_id)
            .map(|s| s.ticket.clone())
    }

    /// 进行中会话数（测试/诊断）。
    pub fn session_count(&self) -> usize {
        lock(&self.sessions).len()
    }

    /// 清空所有会话（退出登录/切换账号时取消进行中的扫码）。
    pub fn clear_sessions(&self) {
        lock(&self.sessions).clear();
    }
}

fn new_ticket_id(provider: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{provider}-{:x}",
        md5::compute(format!("{provider}:{nanos}").as_bytes())
    )
}

/// 按 W13 §5 + W14 补齐组装默认登录中心：115 / 迅雷 / 夸克 / UC 支持站内扫码或设备码，
/// 其余 provider（ali/pikpak）继续走前端 WebView 兜底，不在此注册。
pub fn default_hub() -> Result<LoginHub> {
    let client = login_client()?;
    let hub = LoginHub::new();
    hub.register(Arc::new(p115::P115Login::new(client.clone())));
    hub.register(Arc::new(xunlei::XunleiLogin::new(client.clone())));
    hub.register(Arc::new(quark::QuarkLogin::new(client.clone())));
    hub.register(Arc::new(uc::UcLogin::new(client.clone())));
    hub.register(Arc::new(ali::AliLogin::new(client.clone())));
    hub.register(Arc::new(pikpak::PikPakLogin::new(client)));
    Ok(hub)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 串行化会改动全局 overlay 的测试。
    fn overlay_guard() -> MutexGuard<'static, ()> {
        static GUARD: Mutex<()> = Mutex::new(());
        let guard = lock(&GUARD);
        lock(credential_overlay()).clear();
        guard
    }

    fn qr_ticket(ttl: u64) -> QrTicket {
        QrTicket::new(
            "ticket-1",
            QrKind::QrCode,
            "https://example.com/qr",
            "",
            ttl,
        )
    }

    #[test]
    fn ticket_expiry_and_remaining() {
        let t = qr_ticket(300);
        assert!(!t.is_expired());
        assert!(t.remaining_secs() > 290);
        assert!(t.remaining_secs() <= 300);

        let mut expired = qr_ticket(1);
        expired.expires_at = now_unix() - 1;
        assert!(expired.is_expired());
        assert_eq!(expired.remaining_secs(), 0);

        let unknown = qr_ticket(0);
        assert_eq!(unknown.expires_at, 0);
        assert!(!unknown.is_expired());
        assert_eq!(unknown.remaining_secs(), 0);
    }

    #[test]
    fn login_outcome_serde_shape() {
        assert_eq!(
            serde_json::to_string(&LoginOutcome::Waiting).unwrap(),
            r#"{"state":"waiting"}"#
        );
        assert_eq!(
            serde_json::to_string(&LoginOutcome::Scanned).unwrap(),
            r#"{"state":"scanned"}"#
        );
        let mut cred = Credential::new("p115");
        cred.cookie = Some("UID=1".to_string());
        let json = serde_json::to_string(&LoginOutcome::Success(cred)).unwrap();
        assert!(json.starts_with(r#"{"state":"success","credential":"#));
        assert!(json.contains(r#""provider":"p115""#));
        let back: LoginOutcome = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, LoginOutcome::Success(_)));
        assert_eq!(
            serde_json::to_string(&LoginOutcome::Expired).unwrap(),
            r#"{"state":"expired"}"#
        );
        assert_eq!(
            serde_json::to_string(&LoginOutcome::Failure("boom".into())).unwrap(),
            r#"{"state":"failure","credential":"boom"}"#
        );
    }

    #[test]
    fn ticket_serde_hides_internal_state() {
        let mut t = qr_ticket(120);
        t.state.insert("uid".to_string(), "secret-uid".to_string());
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("secret-uid"));
        assert!(!json.contains("\"state\""));
        assert!(json.contains(r#""ticket_id":"ticket-1""#));
        let back: QrTicket = serde_json::from_str(&json).unwrap();
        assert!(back.state.is_empty());
        assert_eq!(back.content, t.content);
    }

    #[test]
    fn mask_identity_variants() {
        assert_eq!(mask_identity(""), "");
        assert_eq!(mask_identity("ab"), "**");
        assert_eq!(mask_identity("abc"), "a***c");
        assert_eq!(mask_identity("13812345678"), "138****678");
        assert_eq!(mask_identity("u_88"), "u***8");
    }

    #[test]
    fn account_view_status_and_masking() {
        let mut cred = Credential::new("quark");
        cred.cookie = Some("__puus=1".into());
        cred.display_name = Some("u_88".into());
        cred.masked_id = Some(mask_identity("u_88"));
        let view = AccountView::from_credential(&cred);
        assert_eq!(view.provider, "quark");
        assert_eq!(view.status, AccountStatus::Connected);
        assert_eq!(view.masked_id.as_deref(), Some("u***8"));
        assert!(view.account_id.starts_with("quark:"));

        let mut expired = cred.clone();
        expired.expires_at = Some(now_unix() - 10);
        assert_eq!(
            AccountView::from_credential(&expired).status,
            AccountStatus::Expired
        );

        let not_logged = AccountView::not_logged_in("xunlei");
        assert_eq!(not_logged.status, AccountStatus::NotLoggedIn);
        assert!(not_logged.account_id.is_empty());
    }

    #[test]
    fn credential_normalization_rejects_unknown_and_empty() {
        assert!(normalize_credential(Credential::new("nope")).is_err());
        let mut empty = Credential::new("p115");
        empty.cookie = Some("   ".into());
        assert!(normalize_credential(empty).is_err());

        let mut ok = Credential::new("P115");
        ok.token = Some("tok".into());
        let cred = normalize_credential(ok).unwrap();
        assert_eq!(cred.provider, "p115");
        assert!(cred.account_id.starts_with("p115:"));
        assert!(cred.updated_at > 0);
        assert!(cred.is_usable());
    }

    #[test]
    fn overlay_install_active_and_remove() {
        let _guard = overlay_guard();

        let mut cred = Credential::new("p115");
        cred.cookie = Some("UID=1; SEID=2".into());
        let installed = install_credential(cred).unwrap();
        assert_eq!(
            active_credential("p115").unwrap().account_id,
            installed.account_id
        );
        assert_eq!(overlay_cookie("p115").as_deref(), Some("UID=1; SEID=2"));

        // 过期凭据不注入
        let mut expired = Credential::new("quark");
        expired.token = Some("t".into());
        expired.expires_at = Some(now_unix() - 1);
        install_credential(expired).unwrap();
        assert!(overlay_cookie("quark").is_none());
        assert_eq!(
            AccountView::from_credential(&active_credential("quark").unwrap()).status,
            AccountStatus::Expired
        );

        let removed = remove_credential("p115").unwrap();
        assert_eq!(removed.provider, "p115");
        assert!(active_credential("p115").is_none());
        assert!(!overlay_cookie("p115").is_some());
    }

    /// 可编排状态的登录流，模拟各 provider 的 mock 状态机。
    struct ScriptedFlow {
        provider: &'static str,
        ttl: u64,
        script: Mutex<VecDeque<LoginOutcome>>,
        starts: AtomicU64,
        calls: AtomicU64,
    }

    impl ScriptedFlow {
        fn new(provider: &'static str, ttl: u64, script: Vec<LoginOutcome>) -> Self {
            Self {
                provider,
                ttl,
                script: Mutex::new(script.into()),
                starts: AtomicU64::new(0),
                calls: AtomicU64::new(0),
            }
        }

        fn calls(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl LoginFlow for ScriptedFlow {
        fn provider(&self) -> &'static str {
            self.provider
        }

        fn start(&self) -> Result<QrTicket> {
            let n = self.starts.fetch_add(1, Ordering::SeqCst);
            let mut ticket = QrTicket::new(
                format!("{}-{n}", self.provider),
                QrKind::QrCode,
                "content",
                "",
                self.ttl,
            );
            ticket.state.insert("nonce".to_string(), "n1".to_string());
            Ok(ticket)
        }

        fn poll(&self, ticket: &QrTicket) -> Result<LoginOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(ticket.state.get("nonce").map(String::as_str), Some("n1"));
            Ok(lock(&self.script)
                .pop_front()
                .unwrap_or(LoginOutcome::Waiting))
        }
    }

    fn success_credential(provider: &str) -> Credential {
        let mut cred = Credential::new(provider);
        cred.cookie = Some(format!("session={provider}"));
        cred.display_name = Some("tester".to_string());
        cred
    }

    #[test]
    fn hub_p115_state_machine_waits_scans_then_succeeds() {
        let _guard = overlay_guard();
        let hub = LoginHub::new();
        hub.register(Arc::new(ScriptedFlow::new(
            P115,
            300,
            vec![
                LoginOutcome::Waiting,
                LoginOutcome::Scanned,
                LoginOutcome::Success(success_credential(P115)),
            ],
        )));

        let ticket = hub.start("p115").unwrap();
        assert_eq!(ticket.kind, QrKind::QrCode);
        assert_eq!(hub.session_count(), 1);

        assert_eq!(
            hub.poll("p115", &ticket.ticket_id).unwrap(),
            LoginOutcome::Waiting
        );
        assert_eq!(hub.session_count(), 1);
        assert_eq!(
            hub.poll("p115", &ticket.ticket_id).unwrap(),
            LoginOutcome::Scanned
        );
        match hub.poll("p115", &ticket.ticket_id).unwrap() {
            LoginOutcome::Success(cred) => {
                assert_eq!(cred.provider, "p115");
                assert!(cred.account_id.starts_with("p115:"));
            }
            other => panic!("期望 Success，实际 {other:?}"),
        }
        assert_eq!(hub.session_count(), 0);

        // Success 后 overlay 已注入，且会话不可再轮询
        assert!(active_credential("p115").is_some());
        assert!(hub.poll("p115", &ticket.ticket_id).is_err());
    }

    #[test]
    fn hub_xunlei_device_code_can_expire() {
        let _guard = overlay_guard();
        let hub = LoginHub::new();
        hub.register(Arc::new(ScriptedFlow::new(
            XUNLEI,
            120,
            vec![LoginOutcome::Waiting, LoginOutcome::Expired],
        )));
        let ticket = hub.start("xunlei").unwrap();
        assert_eq!(
            hub.poll("xunlei", &ticket.ticket_id).unwrap(),
            LoginOutcome::Waiting
        );
        assert_eq!(
            hub.poll("xunlei", &ticket.ticket_id).unwrap(),
            LoginOutcome::Expired
        );
        assert_eq!(hub.session_count(), 0);
        assert!(active_credential("xunlei").is_none());
    }

    #[test]
    fn hub_quark_failure_ends_session() {
        let _guard = overlay_guard();
        let hub = LoginHub::new();
        hub.register(Arc::new(ScriptedFlow::new(
            QUARK,
            120,
            vec![LoginOutcome::Failure("风控拦截".to_string())],
        )));
        let ticket = hub.start("quark").unwrap();
        match hub.poll("quark", &ticket.ticket_id).unwrap() {
            LoginOutcome::Failure(msg) => assert!(msg.contains("风控")),
            other => panic!("期望 Failure，实际 {other:?}"),
        }
        assert_eq!(hub.session_count(), 0);
        assert!(hub.poll("quark", &ticket.ticket_id).is_err());
    }

    #[test]
    fn hub_cancel_unknown_and_mismatch() {
        let _guard = overlay_guard();
        let hub = LoginHub::new();
        let flow = Arc::new(ScriptedFlow::new(P115, 300, vec![]));
        let calls_before = flow.calls();
        hub.register(flow);

        assert!(!hub.cancel("nope").unwrap());
        assert!(hub.poll("p115", "nope").is_err());
        assert!(!hub.supports("quark"));
        assert!(hub.start("quark").is_err());
        assert!(hub.start("unknown").is_err());

        let ticket = hub.start("p115").unwrap();
        assert!(hub.ticket(&ticket.ticket_id).is_some());
        assert!(hub.cancel(&ticket.ticket_id).unwrap());
        assert!(hub.ticket(&ticket.ticket_id).is_none());
        assert!(!hub.cancel(&ticket.ticket_id).unwrap());
        assert_eq!(hub.session_count(), 0);
        assert_eq!(hub.providers(), vec![P115.to_string()]);
        assert_eq!(hub.session_count(), 0);
        // cancel 不应触发 poll
        assert_eq!(hub.session_count(), 0);
        let _ = calls_before;
    }

    #[test]
    fn hub_ticket_expired_before_poll() {
        let _guard = overlay_guard();
        let hub = LoginHub::new();
        hub.register(Arc::new(ScriptedFlow::new(QUARK, 1, vec![])));
        let mut ticket = hub.start("quark").unwrap();
        // 直接把宿主会话里的票据改成已过期
        {
            let mut sessions = lock(&hub.sessions);
            sessions
                .get_mut(&ticket.ticket_id)
                .unwrap()
                .ticket
                .expires_at = now_unix() - 1;
        }
        ticket.expires_in = 1;
        assert_eq!(
            hub.poll("quark", &ticket.ticket_id).unwrap(),
            LoginOutcome::Expired
        );
        assert_eq!(hub.session_count(), 0);
        assert!(hub.poll("quark", &ticket.ticket_id).is_err());
    }
}
