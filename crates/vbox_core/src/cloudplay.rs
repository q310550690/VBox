//! 云盘分享 → 播放直链（W14 P3：搜索结果播放链路）。
//!
//! 搜索页只有「分享链接 + 提取码」，本模块用 W13 登录凭据（overlay 中的活跃账号）
//! 把分享展开成目录、挑第一个视频文件，再取该 provider 的直链与防盗链请求头：
//!
//! - 115 / 阿里 / 迅雷 / PikPak：分享直链端点已就绪；
//! - 夸克 / UC：上游分享播放需要先转存到个人盘，本轮返回可操作错误（P3 灰度）；
//! - 磁力 / ED2K：不解析（W14 §2.2 明确不承诺可播放）。
//!
//! 隐私：本模块不打印分享链接、提取码或直链，错误文案不包含凭据。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use crate::cloudsearch::SearchProvider;
use crate::error::{CoreError, Result};
use crate::http::client_for;
use crate::model::Rules;
use crate::pan::ali::{AliClient, API_BASE as ALI_API_BASE, REFERER as ALI_REFERER, UA as ALI_UA};
use crate::pan::login::{self, ALI, P115 as P115_KEY, PIKPAK, QUARK, UC, XUNLEI};
use crate::pan::p115::{is_video_name, P115Client, UA as P115_UA, WEB_BASE as P115_WEB_BASE};
use crate::pan::pikpak::PikPakClient;
use crate::pan::quark::UA as QUARK_UA;
use crate::pan::uc::UA as UC_UA;
use crate::pan::xunlei::{
    parse_share as parse_xunlei_share, ShareRef, XunleiDrive, API_BASE as XUNLEI_API_BASE,
    AUTH_BASE as XUNLEI_AUTH_BASE, PIKPAK_API_BASE, PIKPAK_AUTH_BASE, PIKPAK_SHARE_WEB,
    UA as XUNLEI_UA, XUNLEI_CLIENT_ID, XUNLEI_CLIENT_SECRET,
};
use crate::pan::{parse_share_link, PanEntry, PanProvider, ShareLink};

/// 115 防盗链 Referer。
const P115_REFERER: &str = "https://115.com/";
/// 夸克防盗链 Referer。
const QUARK_REFERER: &str = "https://pan.quark.cn/";
/// UC 防盗链 Referer。
const UC_REFERER: &str = "https://drive.uc.cn/";
/// 迅雷分享页 Referer。
const XUNLEI_REFERER: &str = "https://pan.xunlei.com/s/";
/// 目录树最大下探深度（与 `pushshare` 家族一致）。
const MAX_DEPTH: u32 = 3;
/// 单次解析最多浏览的目录数，避免异常分享撑爆请求。
const MAX_DIRS: usize = 200;

/// 一次「分享 → 直链」解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPlay {
    /// provider 键（`p115` / `quark` / `uc` / `ali` / `xunlei` / `pikpak`）。
    pub provider: String,
    /// 选中的视频文件名。
    pub file_name: String,
    /// 直链（仅进程内/前端播放器使用，不写日志）。
    pub url: String,
    /// 播放器需要附带的请求头（防盗链 UA/Referer）。
    pub headers: BTreeMap<String, String>,
}

/// 把分享链接解析成可播放直链（使用 overlay 中的活跃登录凭据）。
///
/// `passcode` 为空时用分享链接自带的提取码。未登录、磁力/ED2K、未知分享格式都返回
/// 可操作的 `CoreError::Config`，由 Tauri 层脱敏后展示给用户。
pub fn resolve_play(share_url: &str, passcode: Option<&str>) -> Result<ResolvedPlay> {
    let detected = crate::cloudsearch::detect::detect_share(share_url).ok_or_else(|| {
        CoreError::Config("无法识别的分享链接（支持 115/夸克/UC/阿里/迅雷/PikPak）".to_string())
    })?;
    let key = provider_key(detected.provider)
        .ok_or_else(|| CoreError::Config("磁力/ED2K 或其它网盘暂不支持直接播放".to_string()))?;
    let credential = login::overlay_active(key).ok_or_else(|| {
        CoreError::Config(format!(
            "播放该资源需要先登录「{}」：请在云盘首页右上角「账号管理」登录后重试",
            display_name(key)
        ))
    })?;
    let passcode = passcode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| detected.password.clone())
        .unwrap_or_default();
    resolve_with(&detected.share_url, &passcode, key, &credential)
}

/// 用显式凭据解析（供测试或未来「指定账号播放」复用）。
pub fn resolve_with(
    share_url: &str,
    passcode: &str,
    provider: &str,
    credential: &login::Credential,
) -> Result<ResolvedPlay> {
    match provider {
        P115_KEY => resolve_p115(share_url, passcode, credential),
        QUARK => Err(transfer_needed("夸克")),
        UC => Err(transfer_needed("UC")),
        ALI => resolve_ali(share_url, passcode, credential),
        XUNLEI => resolve_xunlei(share_url, passcode, credential),
        PIKPAK => resolve_pikpak(share_url, passcode, credential),
        _ => Err(CoreError::Config("暂不支持该网盘的直接播放".to_string())),
    }
}

/// 搜索 provider → 登录 provider 键。
pub fn provider_key(provider: SearchProvider) -> Option<&'static str> {
    match provider {
        SearchProvider::P115 => Some(P115_KEY),
        SearchProvider::Quark => Some(QUARK),
        SearchProvider::Uc => Some(UC),
        SearchProvider::Aliyun => Some(ALI),
        SearchProvider::Xunlei => Some(XUNLEI),
        SearchProvider::PikPak => Some(PIKPAK),
        _ => None,
    }
}

fn display_name(key: &str) -> &'static str {
    match key {
        P115_KEY => "115 网盘",
        QUARK => "夸克网盘",
        UC => "UC 网盘",
        ALI => "阿里云盘",
        XUNLEI => "迅雷云盘",
        PIKPAK => "PikPak",
        _ => "网盘",
    }
}

fn transfer_needed(name: &str) -> CoreError {
    CoreError::Config(format!(
        "{name}分享需要先转存到我的网盘才能播放（W14 P3 灰度）；可点「打开原页」在网页播放"
    ))
}

fn no_video() -> CoreError {
    CoreError::Config("分享内没有找到可播放的视频文件".to_string())
}

fn cookie_of(credential: &login::Credential, name: &str) -> Result<String> {
    credential
        .cookie
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CoreError::Config(format!(
                "{name}凭据缺少 Cookie：请在云盘首页右上角「账号管理」重新登录"
            ))
        })
}

fn token_of(credential: &login::Credential, name: &str) -> Result<String> {
    credential
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CoreError::Config(format!(
                "{name}凭据缺少 access token：请在云盘首页右上角「账号管理」重新登录"
            ))
        })
}

fn pan_link(share_url: &str, passcode: &str, expected: PanProvider) -> Result<ShareLink> {
    let mut link = parse_share_link(share_url)
        .ok_or_else(|| CoreError::Config("分享链接格式无法识别".to_string()))?;
    if link.provider != expected {
        return Err(CoreError::Config("分享链接与网盘类型不匹配".to_string()));
    }
    if !passcode.trim().is_empty() {
        link.passcode = passcode.trim().to_string();
    }
    Ok(link)
}

fn resolve_p115(
    share_url: &str,
    passcode: &str,
    credential: &login::Credential,
) -> Result<ResolvedPlay> {
    let name = display_name(P115_KEY);
    let cookie = cookie_of(credential, name)?;
    let link = pan_link(share_url, passcode, PanProvider::P115)?;
    let client = client_for(&Rules::default())?;
    let drive = P115Client::with_base(client, &cookie, P115_WEB_BASE);
    let video = pick_first_video(|fid| {
        let mut current = link.clone();
        current.sub_fid = fid.to_string();
        drive.browse(&current).map(|browse| browse.files)
    })?
    .ok_or_else(no_video)?;
    let url = drive.download_url(&link.share_id, &link.passcode, &video.fid)?;
    Ok(resolved(P115_KEY, &video.name, url, headers_for(P115_KEY)))
}

fn resolve_ali(
    share_url: &str,
    passcode: &str,
    credential: &login::Credential,
) -> Result<ResolvedPlay> {
    let name = display_name(ALI);
    let cookie = cookie_of(credential, name)?;
    let link = pan_link(share_url, passcode, PanProvider::Ali)?;
    let client = client_for(&Rules::default())?;
    let drive = AliClient::with_base(client, &cookie, ALI_API_BASE);
    let video = pick_first_video(|fid| {
        let mut current = link.clone();
        current.sub_fid = fid.to_string();
        drive.browse(&current).map(|browse| browse.files)
    })?
    .ok_or_else(no_video)?;
    let token = drive.share_token(&link.share_id, &link.passcode)?;
    let url = drive.download_url(&link.share_id, &video.fid, &token.share_token)?;
    Ok(resolved(ALI, &video.name, url, headers_for(ALI)))
}

fn resolve_xunlei(
    share_url: &str,
    passcode: &str,
    credential: &login::Credential,
) -> Result<ResolvedPlay> {
    let name = display_name(XUNLEI);
    let token = token_of(credential, name)?;
    let mut share = xunlei_share(share_url, passcode, false)?;
    if !passcode.trim().is_empty() {
        share.passcode = passcode.trim().to_string();
    }
    let client = client_for(&Rules::default())?;
    let drive = XunleiDrive::with_bases(
        client,
        &token,
        XUNLEI_API_BASE,
        XUNLEI_AUTH_BASE,
        "迅雷",
        XUNLEI_CLIENT_ID,
        XUNLEI_CLIENT_SECRET,
    );
    let video = pick_first_video(|fid| {
        drive
            .share_files(&share.share_id, &share.passcode, fid)
            .map(|(_title, files)| files)
    })?
    .ok_or_else(no_video)?;
    let share_token = drive.share_token(&share.share_id, &share.passcode)?;
    let url = drive.download_url(
        &share.share_id,
        &share.passcode,
        &share_token.pass_code_token,
        &video.fid,
    )?;
    Ok(resolved(XUNLEI, &video.name, url, headers_for(XUNLEI)))
}

fn resolve_pikpak(
    share_url: &str,
    passcode: &str,
    credential: &login::Credential,
) -> Result<ResolvedPlay> {
    let name = display_name(PIKPAK);
    let token = token_of(credential, name)?;
    let mut share = xunlei_share(share_url, passcode, true)?;
    if !passcode.trim().is_empty() {
        share.passcode = passcode.trim().to_string();
    }
    let client = client_for(&Rules::default())?;
    let drive = PikPakClient::with_base(client, &token, PIKPAK_API_BASE, PIKPAK_AUTH_BASE);
    let video = pick_first_video(|fid| {
        drive
            .share_files(&share.share_id, &share.passcode, fid)
            .map(|(_title, files)| files)
    })?
    .ok_or_else(no_video)?;
    let share_token = drive.share_token(&share.share_id, &share.passcode)?;
    let pass_code_token = share_token
        .get("pass_code_token")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let url = drive.download_url(
        &share.share_id,
        &share.passcode,
        pass_code_token,
        &video.fid,
    )?;
    Ok(resolved(PIKPAK, &video.name, url, headers_for(PIKPAK)))
}

/// 解析迅雷/PikPak 分享链接，并校验域名形态。
fn xunlei_share(share_url: &str, _passcode: &str, pikpak: bool) -> Result<ShareRef> {
    let share = parse_xunlei_share(share_url)
        .ok_or_else(|| CoreError::Config("分享链接格式无法识别".to_string()))?;
    if pikpak != share.url.starts_with(PIKPAK_SHARE_WEB) {
        return Err(CoreError::Config("分享链接与网盘类型不匹配".to_string()));
    }
    Ok(share)
}

fn resolved(
    provider: &str,
    file_name: &str,
    url: String,
    headers: BTreeMap<String, String>,
) -> ResolvedPlay {
    ResolvedPlay {
        provider: provider.to_string(),
        file_name: file_name.to_string(),
        url,
        headers,
    }
}

/// 各 provider 播放器需要的请求头（选择器与 `pushshare` 家族保持一致）。
fn headers_for(provider: &str) -> BTreeMap<String, String> {
    let (ua, referer) = match provider {
        P115_KEY => (P115_UA, P115_REFERER),
        QUARK => (QUARK_UA, QUARK_REFERER),
        UC => (UC_UA, UC_REFERER),
        ALI => (ALI_UA, ALI_REFERER),
        XUNLEI => (XUNLEI_UA, XUNLEI_REFERER),
        _ => (XUNLEI_UA, PIKPAK_SHARE_WEB),
    };
    let mut headers = BTreeMap::new();
    headers.insert("User-Agent".to_string(), ua.to_string());
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

/// 浅层 BFS 找第一个视频文件（先看根，再按目录深度下探）。
fn pick_first_video<F>(mut browse: F) -> Result<Option<PanEntry>>
where
    F: FnMut(&str) -> Result<Vec<PanEntry>>,
{
    let mut queue: VecDeque<(String, u32)> = VecDeque::from([(String::new(), 0)]);
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut listed = 0usize;
    while let Some((fid, depth)) = queue.pop_front() {
        if !visited.insert(fid.clone()) {
            continue;
        }
        if listed >= MAX_DIRS {
            break;
        }
        listed += 1;
        let entries = browse(&fid)?;
        if let Some(video) = entries
            .iter()
            .find(|entry| !entry.is_dir && is_video_name(&entry.name))
        {
            return Ok(Some(video.clone()));
        }
        for entry in &entries {
            if entry.is_dir && depth < MAX_DEPTH {
                queue.push_back((entry.fid.clone(), depth + 1));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(fid: &str, name: &str, is_dir: bool) -> PanEntry {
        PanEntry {
            fid: fid.to_string(),
            name: name.to_string(),
            is_dir,
            size: 0,
            thumbnail: String::new(),
        }
    }

    #[test]
    fn provider_key_covers_six_cloud_providers() {
        for (provider, expected) in [
            (SearchProvider::P115, P115_KEY),
            (SearchProvider::Quark, QUARK),
            (SearchProvider::Uc, UC),
            (SearchProvider::Aliyun, ALI),
            (SearchProvider::Xunlei, XUNLEI),
            (SearchProvider::PikPak, PIKPAK),
        ] {
            assert_eq!(provider_key(provider), Some(expected));
        }
        assert_eq!(provider_key(SearchProvider::Magnet), None);
        assert_eq!(provider_key(SearchProvider::Ed2k), None);
    }

    #[test]
    fn pick_first_video_walks_into_subdirectory() {
        let mut call = 0;
        let video = pick_first_video(|fid| {
            call += 1;
            Ok(match fid {
                "" => vec![
                    entry("d1", "第 1 季", true),
                    entry("f0", "readme.txt", false),
                ],
                "d1" => vec![entry("v1", "第 1 集.mp4", false)],
                _ => Vec::new(),
            })
        })
        .unwrap()
        .expect("应找到视频");
        assert_eq!(video.fid, "v1");
        assert_eq!(call, 2);
    }

    #[test]
    fn pick_first_video_respects_depth_and_returns_none() {
        let video = pick_first_video(|fid| {
            Ok(match fid {
                "" => vec![entry("d1", "a", true)],
                "d1" => vec![entry("d2", "b", true)],
                "d2" => vec![entry("d3", "c", true)],
                "d3" => vec![entry("d4", "d", true)],
                "d4" => vec![entry("v1", "deep.mp4", false)],
                _ => Vec::new(),
            })
        })
        .unwrap();
        assert!(video.is_none(), "深于 MAX_DEPTH 的目录不应再下探");
    }

    #[test]
    fn headers_include_user_agent_and_referer() {
        for provider in [P115_KEY, QUARK, UC, ALI, XUNLEI, PIKPAK] {
            let headers = headers_for(provider);
            assert!(headers.get("User-Agent").is_some(), "{provider} 缺 UA");
            assert!(headers.get("Referer").is_some(), "{provider} 缺 Referer");
        }
        assert_eq!(
            headers_for(ALI).get("Referer").map(String::as_str),
            Some(ALI_REFERER)
        );
    }

    #[test]
    fn resolve_rejects_magnet_and_garbage() {
        let magnet = resolve_play("magnet:?xt=urn:btih:deadbeef", None).unwrap_err();
        assert!(matches!(magnet, CoreError::Config(_)));
        let garbage = resolve_play("not a share link", None).unwrap_err();
        assert!(matches!(garbage, CoreError::Config(_)));
    }
}
