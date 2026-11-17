//! 网盘账号登录命令（W13 D6–D8）。
//!
//! 命令与 W13 §4 / W14 §7.2 对齐：
//! - 协议登录：`login_start` / `login_poll` / `login_cancel`（115、夸克、迅雷）；
//! - WebView 兜底：`open_web_login`（阿里 / UC / PikPak，夸克与 115 也可作为兜底）；
//! - 手动：`login_manual`；
//! - 账号管理：`accounts_list` / `credentials_list` / `account_remove` /
//!   `credentials_clear` / `credentials_verify`。
//!
//! 安全边界：
//! - 返回值只含 [`AccountView`] 摘要（账号掩码/状态/更新时间），**不含 cookie/token 明文**；
//! - 登录票据内部状态（uid/sign/device_code）声明为 `#[serde(skip_serializing)]`，不返前端；
//! - 凭据落盘由 [`crate::pan_credentials::PanCredentialStore`]（Windows DPAPI）负责，
//!   cookie/token 不进日志。

use serde::Serialize;
use tauri::{State, WebviewUrl, WebviewWindowBuilder};

use vbox_core::pan::login::{
    ALI, AccountView, Credential, LoginOutcome, P115, PIKPAK, QrTicket, QUARK, UC, XUNLEI,
    require_provider,
};

use crate::commands::AppState;
use crate::pan_credentials::PanCredentialStore;

/// 登录轮询结果（脱敏）：成功时只给账号摘要，cookie/token 留在后端。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LoginPollResult {
    /// 等待扫码 / 等待设备确认。
    Waiting,
    /// 已扫码，等待确认。
    Scanned,
    /// 登录成功，凭据已落盘。
    Success { account: AccountView },
    /// 票据过期。
    Expired,
    /// 失败原因（不含敏感信息）。
    Failure { message: String },
}

impl LoginPollResult {
    fn from_outcome(
        outcome: LoginOutcome,
        store: &PanCredentialStore,
    ) -> Result<LoginPollResult, String> {
        Ok(match outcome {
            LoginOutcome::Waiting => LoginPollResult::Waiting,
            LoginOutcome::Scanned => LoginPollResult::Scanned,
            LoginOutcome::Success(cred) => {
                let account = store.upsert(cred)?;
                LoginPollResult::Success { account }
            }
            LoginOutcome::Expired => LoginPollResult::Expired,
            LoginOutcome::Failure(message) => LoginPollResult::Failure { message },
        })
    }
}

/// 单个 provider 的登录能力（前端账号页据此渲染「扫码登录」或「网页授权」）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginProviderView {
    pub provider: &'static str,
    /// 是否支持协议登录（`login_start`）。
    pub protocol: bool,
    /// 是否有网页登录兜底（`open_web_login`）。
    pub web_login: bool,
}

/* ---------------- 协议登录 ---------------- */

/// 开始一次登录：返回二维码 / 设备码票据（不含任何凭据）。
#[tauri::command]
pub fn login_start(state: State<'_, AppState>, provider: String) -> Result<QrTicket, String> {
    let provider = require_provider(&provider).map_err(|e| e.to_string())?;
    state
        .pan_login
        .start(&provider)
        .map_err(|e| e.to_string())
}

/// 轮询登录状态；成功时把凭据加密落盘并返回账号摘要（前端轮询到终态即停）。
#[tauri::command]
pub fn login_poll(
    state: State<'_, AppState>,
    provider: String,
    ticket_id: String,
) -> Result<LoginPollResult, String> {
    let provider = require_provider(&provider).map_err(|e| e.to_string())?;
    let outcome = state
        .pan_login
        .poll(&provider, &ticket_id)
        .map_err(|e| e.to_string())?;
    LoginPollResult::from_outcome(outcome, &state.pan_credentials)
}

/// 取消一次进行中的登录（关闭面板 / 刷新二维码）；返回是否确有会话被取消。
#[tauri::command]
pub fn login_cancel(state: State<'_, AppState>, ticket_id: String) -> Result<bool, String> {
    state.pan_login.cancel(&ticket_id).map_err(|e| e.to_string())
}

/// 手动写入凭据（第三方工具导出的 cookie/token，或无法扫码时的兜底）。
#[tauri::command]
pub fn login_manual(
    state: State<'_, AppState>,
    provider: String,
    cookie: Option<String>,
    token: Option<String>,
    refresh_token: Option<String>,
    display_name: Option<String>,
) -> Result<AccountView, String> {
    let provider = require_provider(&provider).map_err(|e| e.to_string())?;
    let non_empty = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
    let mut cred = Credential::new(provider);
    cred.cookie = non_empty(cookie);
    cred.token = non_empty(token);
    cred.refresh_token = non_empty(refresh_token);
    cred.display_name = non_empty(display_name);
    if cred.cookie.is_none() && cred.token.is_none() {
        return Err("请至少提供 cookie 或 token".into());
    }
    state.pan_credentials.upsert(cred)
}

/* ---------------- 账号管理 ---------------- */

/// 已保存账号列表（W14 命名）。
#[tauri::command]
pub fn accounts_list(state: State<'_, AppState>) -> Result<Vec<AccountView>, String> {
    state.pan_credentials.list()
}

/// 已保存账号列表（W13 §4 命名，与 [`accounts_list`] 同义）。
#[tauri::command]
pub fn credentials_list(state: State<'_, AppState>) -> Result<Vec<AccountView>, String> {
    state.pan_credentials.list()
}

/// 删除一个账号（W14 `account_remove`）。
#[tauri::command]
pub fn account_remove(state: State<'_, AppState>, account_id: String) -> Result<bool, String> {
    state.pan_credentials.remove(&account_id)
}

/// 退出某 provider 的全部账号（W13 `credentials_clear`）。
#[tauri::command]
pub fn credentials_clear(state: State<'_, AppState>, provider: String) -> Result<usize, String> {
    state.pan_credentials.remove_provider(&provider)
}

/// 校验某 provider 当前是否有可用凭据（本地过期判断，不发网络请求）。
#[tauri::command]
pub fn credentials_verify(state: State<'_, AppState>, provider: String) -> Result<bool, String> {
    let provider = require_provider(&provider).map_err(|e| e.to_string())?;
    Ok(state.pan_credentials.verify(&provider))
}

/// 各 provider 的登录能力（前端决定展示「扫码登录」还是「网页授权」）。
#[tauri::command]
pub fn login_providers(state: State<'_, AppState>) -> Vec<LoginProviderView> {
    [P115, QUARK, XUNLEI, UC, ALI, PIKPAK]
        .into_iter()
        .map(|provider| LoginProviderView {
            provider,
            protocol: state.pan_login.supports(provider),
            web_login: web_login_url(provider).is_some(),
        })
        .collect()
}

/* ---------------- WebView 兜底 ---------------- */

/// 网页登录入口：打开可见登录窗口，等待用户登录后从 WebView 取回 Cookie 并加密落盘。
///
/// 这是阿里 / UC / PikPak 的唯一可行路径，也作为 115 / 夸克的兜底。
/// 返回 `Ok(None)` 表示用户关闭了窗口（取消）；超时返回错误。
#[tauri::command]
pub async fn open_web_login(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    provider: String,
    timeout_secs: Option<u64>,
) -> Result<Option<AccountView>, String> {
    let provider = require_provider(&provider).map_err(|e| e.to_string())?;
    let url = web_login_url(&provider)
        .ok_or_else(|| format!("{provider} 没有可用的网页登录入口"))?;
    let parsed: tauri::Url = url
        .parse()
        .map_err(|e| format!("登录页 URL 非法：{url}（{e}）"))?;
    let timeout = timeout_secs.unwrap_or(300).clamp(30, 900);

    let label = format!(
        "pan-login-{provider}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let title = format!("登录 {}", provider_display_name(&provider));
    let window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
        .title(title)
        .inner_size(520.0, 760.0)
        .build()
        .map_err(|e| format!("打开登录窗口失败：{e}"))?;

    // 建窗必须在 async 上下文；轮询 cookies 是阻塞代码，丢给 spawn_blocking。
    let window_handle = window.clone();
    let provider_owned = provider.to_string();
    let polled = tauri::async_runtime::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(800));
            let cookies = match window_handle.cookies() {
                Ok(cookies) => cookies,
                // 用户关闭窗口：按取消处理。
                Err(_) => return Ok(None),
            };
            let pairs: Vec<String> = cookies
                .iter()
                .map(|c| format!("{}={}", c.name(), c.value()))
                .collect();
            if session_cookie_ready(&provider_owned, &pairs) {
                return Ok(Some(pairs));
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("等待网页登录超时（{timeout} 秒）"));
            }
        }
    })
    .await
    .map_err(|e| format!("网页登录任务失败：{e}"))?;

    // 无论成功/取消都关闭登录窗口。
    let _ = window.close();

    let Some(pairs) = polled? else {
        return Ok(None);
    };
    let display_name = format!("{}（网页授权）", provider_display_name(&provider));
    let mut cred = Credential::new(provider);
    cred.cookie = Some(pairs.join("; "));
    cred.display_name = Some(display_name);
    let account = state.pan_credentials.upsert(cred)?;
    Ok(Some(account))
}

/// 网页登录地址；`None` 表示该 provider 不支持网页登录。
fn web_login_url(provider: &str) -> Option<&'static str> {
    match provider {
        P115 => Some("https://115.com/?ct=login"),
        QUARK => Some("https://pan.quark.cn/"),
        ALI => Some("https://www.alipan.com/"),
        UC => Some("https://drive.uc.cn/"),
        PIKPAK => Some("https://mypikpak.com/"),
        _ => None,
    }
}

fn provider_display_name(provider: &str) -> &'static str {
    match provider {
        P115 => "115 网盘",
        QUARK => "夸克网盘",
        XUNLEI => "迅雷云盘",
        UC => "UC 网盘",
        ALI => "阿里云盘",
        PIKPAK => "PikPak",
        _ => "网盘",
    }
}

/// 判断 WebView Cookie 里是否已出现该 provider 的登录态 Cookie。
///
/// 只做本地标志位判断（不联网、不校验有效性）；真正的有效性由首次业务请求验证。
fn session_cookie_ready(provider: &str, cookies: &[String]) -> bool {
    let has = |name: &str| {
        cookies
            .iter()
            .any(|c| c.split('=').next().map(|n| n.trim() == name).unwrap_or(false))
    };
    match provider {
        P115 => has("UID") && has("CID"),
        QUARK => has("__pus") || has("__puus"),
        ALI | UC => has("__puus"),
        PIKPAK => has("access_token") || has("__puus"),
        XUNLEI => has("access_token"),
        _ => false,
    }
}
