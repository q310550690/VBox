//! `csp_Kuaikan` 原生实现（「快看」App：登录 + 全量 JSON 接口，AES 信封 + RSA 签名 tiket）。
//!
//! ## 来源（REA 反编译证据）
//!
//! * jar `88c55af0-fan.zip` 的 `com.github.catvod.spider.Kuaikan`：697 行、0
//!   `Method dump skipped`，可直接通读（字符串为明文常量，无混淆）。
//! * 依赖的 jar 内自包含助手：`spider.merge.C0246wA`（OkHttp POST 封装）、
//!   `spider.merge.F5`（`yq(k,v,...)` 建 HashMap）、`spider.merge.nA`（响应回调
//!   接口）、`spider.merge.Zz.M(str)`（视频直链判定，等价于本项目
//!   `crate::parse::is_direct_media_url`）。均未引用宿主 `parser.merge`。
//!
//! ## 协议（从源码逐段还原）
//!
//! 基址 `http://ctlook.facaishiyi.com/api/v1/`，AppId 固定 `1000300001`。
//! 所有请求均为 POST JSON：`{"body": <AES-CBC 密文>, "ticket": <RSA 私钥加密>}`；
//! 头 `j(ts)`：`appId`、`version: 4`、`timestamp`、`token = md5(前缀+"4"+ts+"notice")`、
//! `User-Agent: okhttp/5.0.0-alpha.2`。
//!
//! * 登录：`user/login.do`，body 密钥为固定 `LOGIN_KEY` 的前 16 / 后 16 字节，
//!   ticket = RSA 私钥加密 `LOGIN_KEY`（32 字符）；响应 `{"ticket","body"}`，
//!   ticket 由 RSA 公钥解出 32 字符 AES 密钥（前 16 key、后 16 iv），
//!   body 解开后为 `{"duration": <秒>, "data": <32 字符 SN>}`，SN 有效期 duration。
//! * 分类：`user/genres.do` → `genres[]`（`typeId > 0` 才收录）；
//!   `search/category.do` → 每个频道的 `tags/areas/years` 筛选（桌面端无筛选 UI，
//!   仅保留 DOM 语义，记偏差）。
//! * 首页推荐：`user/genre/115.do` → `subGenres[]` 中 `template == 1` 的 `video[]`。
//! * 分类列表：`search/video.do`，`pageSize=18`，签名含
//!   `page + 18 + typeId + tagId + areaId + yearId + ts`。
//! * 详情：`video/play/{id}.do`（原类 URL 末尾多一个空格，已修剪）；
//!   `data.newPlayback` 用固定 `NEWPLAY_KEY`/`NEWPLAY_IV` 解 AES → 线路数组
//!   `[{name, id, episodes:[{title, url}]}]`；单集地址为
//!   `base64("{title}|{vod_id}|{line_id}|{url}")`（NO_WRAP）。
//! * 播放：把 id 的 base64 解开成四段；`line_id` 经 `user/config.do` 的
//!   `data.newAnalysis[]`（`id → analyses[].url` 列表）找到分析站点，对每个候选
//!   URL POST 密文 `{appId, id, title, url, sign}`，响应 `code==0 && msg=="success"`
//!   时取 `url` 直链；否则若原 `url` 本身是视频直链则直出，否则报解析错误。
//! * 搜索：`search/page.do`，`pageSize=12`，签名含关键字。
//!
//! ## 有意偏差（原类自带问题或桌面端边界）
//!
//! 1. 上游 `ctlook.facaishiyi.com` 已无响应（fetch failed），本家族只能 mock
//!    验收，无真站 `#[ignore]` 用例。
//! 2. 原类 `init` 用 `SharedPreferences` 持久化随机 `umId`；桌面端改为固定常量
//!    `DESKTOP_UMID`（只参与 `user/config.do` 的签名，服务端不校验业务含义）。
//! 3. 原类带 `duration` 缓存 SN；本实现同样按 base+umId 缓存，过期重登。
//! 4. 原类 `filters` 通过异步二次请求聚合；桌面端 `Site` 无筛选 UI，`videos` 固定
//!    `areaId/tagId/yearId = 0`（与用户直接进入分类首页时一致）。
//! 5. 播放无候选中标且原 URL 非直链时，原类返回空串，本实现返回
//!    `catvod.result.parse` 结构化错误（便于宿主提示）。
//! 6. ext 支持 JSON 覆盖 `base` / `um_id` / `rsa_priv` / `rsa_pub`，**仅供测试与镜像**
//!    使用（生产不传即用内置常量）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BigUint, RsaPrivateKey, RsaPublicKey};
use serde_json::{json, Value};

use crate::error::Result;
use crate::http::client_for;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};

/// 家族标识（`profiles.json` 的 api 名）。
pub const API: &str = "csp_Kuaikan";
/// 默认基址。
pub const DEFAULT_BASE: &str = "http://ctlook.facaishiyi.com/api/v1";
/// AppId（原类硬编码）。
const APP_ID: &str = "1000300001";
/// 所有签名串的前缀（原类硬编码）。
const SIGN_PREFIX: &str = "1000300001dc431681b806089dac1153fb13960f87";
/// 登录 body 的固定 AES 密钥串（前 16 / 后 16 字节）。
const LOGIN_KEY: &str = "bf70a456195ae394184b2e0b1b471cae";
/// 详情 `newPlayback` 的固定 AES key / iv。
const NEWPLAY_KEY: &str = "5551e2a82a21a9a1";
const NEWPLAY_IV: &str = "a586c5236206a2cb";
/// 请求 UA（原类 `j()`）。
const UA: &str = "okhttp/5.0.0-alpha.2";
/// 桌面端固定 umId（替代 SharedPreferences 的随机值）。
const DESKTOP_UMID: &str = "vb0xdesktopkuaikan000000";
/// 分类列表每页条数（原类硬编码 18）。
const PAGE_SIZE: u64 = 18;
/// 搜索每页条数（原类硬编码 12）。
const SEARCH_PAGE_SIZE: u64 = 12;

/// 内置 RSA 私钥（PKCS#8，用于 ticket 签名；原类为 2048 位）。
const RSA_PRIV_B64: &str = "MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDitX/rWP6WWc3hDzUcfD8Hbw79yCB7rz18GEkjDapjlUVIa3P8FEQFoOchu7N+vdK6HQNsl2Bql4lvWOYuzRWNbLGQ7mNtMca28iCuPHck0AcJzOs9F6qruPL73eY7Ai7yvzs2IfEdyXSzHP/yXgEOi5ZEBfsBZEPMJw6vfHJVueuD/lh/LkoJ/gXEKMCM/pkIzGpBPd2TsIg8ZdvOGMPy+MOnJb1Qfe0y3QQdc5VPVdZFWoWsM9RAenHmE3d1THb+uNZZZcnM1tV7xYgBbIhgHtXAZnWDWDRBM3xJPR3O9dfBHHt/tbyrZrSwK1IgwFgqfYm/tRbmp4y/1DhIF6mDAgMBAAECggEAIzDjS2gEFNiZ0a6nouVSb1f47sHq8OgR1jp619seMNkR6Rzs4xtON8VzO1REl47lsAgi6O9SgxlEtykIiglBqDNQGNw4SNHqM6nAEuvF3sv/27CYb2JGFuPdq+UVAOHk4b93dH6uS95ipA8DV97psRVP3P1EqkGjGISTjf/2S4IG10Q0UhP4xxiRS2iCtaIq0fbvnrChqYRlYQYpkkRCCH+Q4FNKquSsqmWyCu5yfeQMMek+aT9KgvmCh05UCeqC1x7xPtSneNbpJpLD5zW8CcG7EYFDtIP8b9BPDlud47ZH5Y4/wyQQFHHLOaniTJVvEJZ7cIoXy2gOhvgcHpNryQKBgQDxNNtXnCfVK0JYNhnU5WCOX9NeW9Q79HldhTM5/EqsjA5H21j5S7A/duBSRfX7iou1hyXaa2p+J7rxF9sjNlwHPsKhprskxXaHJ7qU84gSmwORsTQQPmtTsizGwVsM330+Q2ZROaSEfDoUqH9+KBKs6K+FOldcTOrCZ+ILk8werwKBgQDwnQUrKATzRThVdD6Hcgt/F/SAizDmvtiTVvRcGkiV3EtB5Rx8zFNJ9GWrqSAArwIxTWgfYCYsvpUjbJW02Mg0Ufj2+YhjT713O9yViwUq996St9eLwGQO5FDMD8FYi7bRAiZEAeAeIEeveDRsNK15++8VVJVo7b2lzcapp5k3bQKBgQDSiJNHRhqSet6+xgIIDGoZ+1Qv0TFPX5UrZt8OpsK7FshEOhXQCxt8WZN03HHXK9fEC4GjwwxBrwYB+BOjCYiMHmCd3j0M3HoXgDrgViyYKMuVuDk8UG83r5ZbqVuCwfO8i/HbxddueEvtyiD2CZ47ZCIHxKOKAe0K4Mex2UBaKwKBgQDirEouOelP9Kn5dyVhHENQXBTu9DIBb1FAnO5fxjMTWxFa5qhLuYHNbfxKF24atsRuBepNhJryFCkT0jvGv2L8Ry0wHiwqwvhO14obJ3ia3iBEQAVDlt+sV9L2KvGOpTB4/nlmIA4u02I1IBziw02aWYkGo0SOMUo0ZQL+2PEykQKBgAV1uqiJlWuA9uPdFFJPZPL2YoOuTWdWfIwIb/UdsbUuTckCgqPIqUPi3HhiVSunOgcO7HWK3GD3j8k4XEbWG4y+Ik0lLrWzBl728tZe1GENzTf8aapElHlUfFSM4OlrTRBsbMOsQBcJNh7QbyoZBHiZZTbRUY6xUR4DGYkCOksF";
/// 内置 RSA 公钥（X.509，用于解响应 ticket；原类为 1024 位）。
const RSA_PUB_B64: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCiYJ5UFjeEgeUi5ynLKhj5EMF03sawTyAKoPbfUCSZZQ5QdrHhrINggrYxDtB/TWXoqSF+BJP85F0vq8B6gOqFWhK/1PIKe2wKlRRuZSOdUzK/3vzRHmR3J3KGrvNzvwARH8gg8xZjbsalc/gBkcVxSxGd8j8rJ4QhOSFh9F8B2wIDAQAB";

fn parse_err(site: &Site, msg: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, msg).into_core()
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn md5_hex(input: &str) -> String {
    format!("{:x}", md5::compute(input.as_bytes()))
}

fn b64_decode(input: &str) -> Option<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    STANDARD.decode(cleaned.as_bytes()).ok()
}

fn b64_encode(input: &[u8]) -> String {
    STANDARD.encode(input)
}

fn aes_encrypt_b64(key: &str, iv: &str, plain: &str) -> Option<String> {
    type Enc = cbc::Encryptor<aes::Aes128>;
    use aes::cipher::block_padding::Pkcs7;
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    let enc = Enc::new_from_slices(key.as_bytes(), iv.as_bytes()).ok()?;
    Some(b64_encode(&enc.encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes())))
}

fn aes_decrypt_b64(key: &str, iv: &str, body: &str) -> Option<String> {
    type Dec = cbc::Decryptor<aes::Aes128>;
    use aes::cipher::block_padding::Pkcs7;
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    let raw = b64_decode(body)?;
    let dec = Dec::new_from_slices(key.as_bytes(), iv.as_bytes()).ok()?;
    let plain = dec.decrypt_padded_vec_mut::<Pkcs7>(&raw).ok()?;
    String::from_utf8(plain).ok()
}

fn rsa_priv_from_b64(b64: &str) -> Option<RsaPrivateKey> {
    let der = b64_decode(b64)?;
    RsaPrivateKey::from_pkcs8_der(&der).ok()
}

fn rsa_pub_from_b64(b64: &str) -> Option<RsaPublicKey> {
    let der = b64_decode(b64)?;
    RsaPublicKey::from_public_key_der(&der).ok()
}

/// 原类 `KJ`：RSA 私钥加密（PKCS#1 v1.5 type 1 填充），输入超过单块容量时按 245 字节
/// 分块（原类按 256 分块仅用于 2048 位密钥，本实现用 `k - 11` 更安全，单块场景等价）。
fn rsa_private_encrypt_b64(key: &RsaPrivateKey, data: &[u8]) -> Option<String> {
    let k = key.size();
    let chunk = k.saturating_sub(11).max(1);
    let mut out = Vec::new();
    for part in data.chunks(chunk) {
        let mut em = Vec::with_capacity(k);
        em.push(0x00);
        em.push(0x01);
        em.extend(std::iter::repeat(0xffu8).take(k.saturating_sub(3 + part.len())));
        em.push(0x00);
        em.extend_from_slice(part);
        let m = BigUint::from_bytes_be(&em);
        let c = m.modpow(key.d(), key.n());
        let bytes = c.to_bytes_be();
        out.extend(std::iter::repeat(0u8).take(k.saturating_sub(bytes.len())));
        out.extend(bytes);
    }
    Some(b64_encode(&out))
}

/// 原类 `M`：RSA 公钥解密（私钥加密的逆运算，手工去 PKCS#1 type 1 填充）。
fn rsa_public_decrypt(key: &RsaPublicKey, body_b64: &str) -> Option<String> {
    let k = key.size();
    let raw = b64_decode(body_b64)?;
    if raw.len() % k != 0 {
        return None;
    }
    let mut out = Vec::new();
    for part in raw.chunks(k) {
        let c = BigUint::from_bytes_be(part);
        let m = c.modpow(key.e(), key.n());
        let em = m.to_bytes_be();
        let padded: Vec<u8> = std::iter::repeat(0u8)
            .take(k.saturating_sub(em.len()))
            .chain(em)
            .collect();
        // type 1：00 01 FF..FF 00 data
        if padded.len() < 11 || padded[0] != 0x00 || padded[1] != 0x01 {
            return None;
        }
        let sep = padded.iter().skip(2).position(|b| *b == 0x00)? + 2;
        out.extend_from_slice(&padded[sep + 1..]);
    }
    String::from_utf8(out).ok()
}

/// ext 覆盖项（仅为测试/镜像服务，生产 ext 为空）。
#[derive(Debug, Clone, Default)]
pub struct ExtConf {
    pub base: Option<String>,
    pub um_id: Option<String>,
    pub rsa_priv: Option<String>,
    pub rsa_pub: Option<String>,
}

fn ext_conf(site: &Site) -> ExtConf {
    let raw = site.ext_string();
    let raw = raw.trim();
    if !raw.starts_with('{') {
        return ExtConf::default();
    }
    let value: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let s = |k: &str| value.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    ExtConf { base: s("base"), um_id: s("um_id"), rsa_priv: s("rsa_priv"), rsa_pub: s("rsa_pub") }
}

/// 已解析的密钥与基址上下文。
struct Keys {
    base: String,
    um_id: String,
    priv_key: RsaPrivateKey,
    pub_key: RsaPublicKey,
}

fn keys(site: &Site) -> Result<Keys> {
    let conf = ext_conf(site);
    let base = conf.base.clone().unwrap_or_else(|| DEFAULT_BASE.to_string());
    let base = base.trim_end_matches('/').to_string();
    let um_id = conf.um_id.clone().unwrap_or_else(|| DESKTOP_UMID.to_string());
    let priv_key = rsa_priv_from_b64(conf.rsa_priv.as_deref().unwrap_or(RSA_PRIV_B64))
        .ok_or_else(|| parse_err(site, "内置 RSA 私钥解析失败"))?;
    let pub_key = rsa_pub_from_b64(conf.rsa_pub.as_deref().unwrap_or(RSA_PUB_B64))
        .ok_or_else(|| parse_err(site, "内置 RSA 公钥解析失败"))?;
    Ok(Keys { base, um_id, priv_key, pub_key })
}

fn sessions() -> &'static Mutex<HashMap<String, (String, u64)>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, (String, u64)>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_sn(cache_key: &str) -> Option<String> {
    let guard = sessions().lock().ok()?;
    let (sn, expire) = guard.get(cache_key)?;
    if now_ms() < *expire {
        Some(sn.clone())
    } else {
        None
    }
}

fn store_sn(cache_key: &str, sn: &str, expire_ms: u64) {
    if let Ok(mut guard) = sessions().lock() {
        guard.insert(cache_key.to_string(), (sn.to_string(), expire_ms));
    }
}

fn headers(ts: u64) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("appId".to_string(), APP_ID.to_string());
    h.insert("version".to_string(), "4".to_string());
    h.insert("timestamp".to_string(), ts.to_string());
    h.insert("token".to_string(), md5_hex(&format!("{SIGN_PREFIX}4{ts}notice")));
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

fn post_envelope(
    cfg: &TvConfig,
    site: &Site,
    keys: &Keys,
    sn: &str,
    path: &str,
    payload: &Value,
) -> Result<Value> {
    let ts = now_ms() / 1000;
    let body = aes_encrypt_b64(&sn[0..16], &sn[16..32], &payload.to_string())
        .ok_or_else(|| parse_err(site, "AES 加密失败"))?;
    let ticket = rsa_private_encrypt_b64(&keys.priv_key, sn.as_bytes())
        .ok_or_else(|| parse_err(site, "RSA 签名失败"))?;
    let envelope = json!({ "body": body, "ticket": ticket });
    let url = format!("{}/{}", keys.base, path.trim_start_matches('/'));
    let client = client_for(&cfg.http)?;
    let mut req = client.post(&url).json(&envelope);
    for (k, v) in headers(ts) {
        req = req.header(k, v);
    }
    let resp = req.send().map_err(|e| parse_err(site, format!("请求失败（{url}）: {e}")))?;
    if !resp.status().is_success() {
        return Err(parse_err(site, format!("HTTP {}（{url}）", resp.status())));
    }
    let text = resp.text().map_err(|e| parse_err(site, format!("读取响应失败: {e}")))?;
    let outer: Value = serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("响应不是 JSON（{url}）: {e}")))?;
    let ticket = outer.get("ticket").and_then(Value::as_str).unwrap_or("");
    let body = outer.get("body").and_then(Value::as_str).unwrap_or("");
    if ticket.is_empty() || body.is_empty() {
        return Err(parse_err(site, "响应缺少 ticket/body"));
    }
    let aes_key = rsa_public_decrypt(&keys.pub_key, ticket)
        .ok_or_else(|| parse_err(site, "响应 ticket 解密失败"))?;
    if aes_key.len() < 32 {
        return Err(parse_err(site, "响应 AES 密钥长度异常"));
    }
    let plain = aes_decrypt_b64(&aes_key[0..16], &aes_key[16..32], body)
        .ok_or_else(|| parse_err(site, "响应 body 解密失败"))?;
    serde_json::from_str(&plain).map_err(|e| parse_err(site, format!("响应明文不是 JSON: {e}")))
}

fn login(cfg: &TvConfig, site: &Site, keys: &Keys) -> Result<String> {
    let ts = now_ms() / 1000;
    let payload = json!({
        "appId": APP_ID,
        "sign": md5_hex(&format!("{SIGN_PREFIX}{ts}notice")),
        "timestamp": ts,
    });
    let body = aes_encrypt_b64(&LOGIN_KEY[0..16], &LOGIN_KEY[16..32], &payload.to_string())
        .ok_or_else(|| parse_err(site, "登录 AES 加密失败"))?;
    let ticket = rsa_private_encrypt_b64(&keys.priv_key, LOGIN_KEY.as_bytes())
        .ok_or_else(|| parse_err(site, "登录 RSA 签名失败"))?;
    let url = format!("{}/user/login.do", keys.base);
    let client = client_for(&cfg.http)?;
    let mut req = client.post(&url).json(&json!({ "body": body, "ticket": ticket }));
    for (k, v) in headers(ts) {
        req = req.header(k, v);
    }
    let resp = req.send().map_err(|e| parse_err(site, format!("登录请求失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(parse_err(site, format!("登录 HTTP {}", resp.status())));
    }
    let text = resp.text().map_err(|e| parse_err(site, format!("登录响应读取失败: {e}")))?;
    let outer: Value = serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("登录响应不是 JSON: {e}")))?;
    let ticket = outer.get("ticket").and_then(Value::as_str).unwrap_or("");
    let body = outer.get("body").and_then(Value::as_str).unwrap_or("");
    let aes_key = rsa_public_decrypt(&keys.pub_key, ticket)
        .ok_or_else(|| parse_err(site, "登录 ticket 解密失败"))?;
    if aes_key.len() < 32 {
        return Err(parse_err(site, "登录 AES 密钥长度异常"));
    }
    let plain = aes_decrypt_b64(&aes_key[0..16], &aes_key[16..32], body)
        .ok_or_else(|| parse_err(site, "登录 body 解密失败"))?;
    let value: Value = serde_json::from_str(&plain)
        .map_err(|e| parse_err(site, format!("登录明文不是 JSON: {e}")))?;
    let sn = value.get("data").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if sn.len() < 32 {
        return Err(parse_err(site, "登录响应缺少 SN"));
    }
    let duration = value.get("duration").and_then(Value::as_u64).unwrap_or(0);
    let expire = now_ms() + duration.saturating_mul(1000).max(60_000);
    let cache_key = format!("{}|{}", keys.base, keys.um_id);
    store_sn(&cache_key, &sn, expire);
    Ok(sn)
}

fn ensure_sn(cfg: &TvConfig, site: &Site, keys: &Keys) -> Result<String> {
    let cache_key = format!("{}|{}", keys.base, keys.um_id);
    if let Some(sn) = cached_sn(&cache_key) {
        return Ok(sn);
    }
    login(cfg, site, keys)
}

fn s_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn id_field(v: &Value, key: &str) -> Option<String> {
    match v.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn item_of(v: &Value) -> Option<VideoItem> {
    let id = id_field(v, "id")?;
    let name = s_field(v, "name")?.to_string();
    Some(VideoItem {
        id,
        name,
        cover: s_field(v, "pic").map(str::to_string),
        remark: s_field(v, "progress").map(str::to_string),
    })
}

fn items_of(value: &Value) -> Vec<VideoItem> {
    value
        .get("results")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(item_of).collect())
        .unwrap_or_default()
}

fn analysis_map(cfg: &TvConfig, site: &Site, keys: &Keys, sn: &str) -> Result<HashMap<String, Vec<String>>> {
    let ts = now_ms() / 1000;
    let payload = json!({
        "adv": 0,
        "appId": APP_ID,
        "nav": 0,
        "sign": md5_hex(&format!("{SIGN_PREFIX}00{}{ts}notice", keys.um_id)),
        "timestamp": ts,
        "umId": keys.um_id,
    });
    let value = post_envelope(cfg, site, keys, sn, "user/config.do", &payload)?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let list = value
        .get("data")
        .and_then(|d| d.get("newAnalysis"))
        .and_then(Value::as_array);
    if let Some(list) = list {
        for line in list {
            let Some(id) = line.get("id").and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            }) else {
                continue;
            };
            let urls: Vec<String> = line
                .get("analyses")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| s_field(a, "url").map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if !urls.is_empty() {
                map.insert(id, urls);
            }
        }
    }
    Ok(map)
}

/// 家族的 `categories`：`user/genres.do` → 只保留 `typeId > 0` 的频道。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let keys = keys(site)?;
    let sn = ensure_sn(cfg, site, &keys)?;
    let ts = now_ms() / 1000;
    let payload = json!({
        "appId": APP_ID,
        "sign": md5_hex(&format!("{SIGN_PREFIX}{ts}notice")),
        "timestamp": ts,
    });
    let value = post_envelope(cfg, site, &keys, &sn, "user/genres.do", &payload)?;
    let mut out = Vec::new();
    if let Some(genres) = value.get("genres").and_then(Value::as_array) {
        for g in genres {
            let Some(type_id) = g.get("typeId").and_then(Value::as_i64).filter(|v| *v > 0) else {
                continue;
            };
            let Some(name) = s_field(g, "name") else {
                continue;
            };
            out.push(Category { type_id: type_id.to_string(), type_name: name.to_string() });
        }
    }
    if out.is_empty() {
        return Err(parse_err(site, "分类响应为空"));
    }
    Ok(out)
}

/// 家族的 `videos`：`search/video.do`（`pageSize=18`）。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let keys = keys(site)?;
    let sn = ensure_sn(cfg, site, &keys)?;
    let ts = now_ms() / 1000;
    let page_num = page.max(1) as u64;
    let tid: u64 = type_id.trim().parse().unwrap_or(0);
    let payload = json!({
        "appId": APP_ID,
        "typeId": tid,
        "areaId": 0,
        "tagId": 0,
        "yearId": 0,
        "page": page_num,
        "pageSize": PAGE_SIZE,
        "sign": md5_hex(&format!("{SIGN_PREFIX}{page_num}{PAGE_SIZE}{tid}000{ts}notice")),
        "timestamp": ts,
    });
    let value = post_envelope(cfg, site, &keys, &sn, "search/video.do", &payload)?;
    Ok(items_of(&value))
}

/// 家族的 `search`：`search/page.do`（`pageSize=12`）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keys = keys(site)?;
    let sn = ensure_sn(cfg, site, &keys)?;
    let ts = now_ms() / 1000;
    let payload = json!({
        "appId": APP_ID,
        "keyword": keyword,
        "page": 1,
        "pageSize": SEARCH_PAGE_SIZE,
        "sign": md5_hex(&format!("{SIGN_PREFIX}{keyword}{ts}notice")),
        "timestamp": ts,
    });
    let value = post_envelope(cfg, site, &keys, &sn, "search/page.do", &payload)?;
    Ok(items_of(&value))
}

/// 家族的 `detail`：`video/play/{id}.do`，`newPlayback` 二次 AES 解密。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let keys = keys(site)?;
    let sn = ensure_sn(cfg, site, &keys)?;
    let ts = now_ms() / 1000;
    let payload = json!({
        "appId": APP_ID,
        "id": id,
        "sign": md5_hex(&format!("{SIGN_PREFIX}{id}{ts}notice")),
        "timestamp": ts,
    });
    let path = format!("video/play/{}.do", id.trim());
    let value = post_envelope(cfg, site, &keys, &sn, &path, &payload)?;
    let data = value
        .get("data")
        .ok_or_else(|| parse_err(site, "详情响应缺少 data"))?;
    let vod_id = id_field(data, "id").unwrap_or_else(|| id.to_string());
    let name = s_field(data, "name").unwrap_or("").to_string();
    if name.is_empty() {
        return Err(parse_err(site, "详情响应缺少片名"));
    }
    let mut groups = Vec::new();
    if let Some(playback) = s_field(data, "newPlayback") {
        let plain = aes_decrypt_b64(NEWPLAY_KEY, NEWPLAY_IV, playback)
            .ok_or_else(|| parse_err(site, "newPlayback 解密失败"))?;
        let lines: Value = serde_json::from_str(&plain)
            .map_err(|e| parse_err(site, format!("newPlayback 不是 JSON: {e}")))?;
        if let Some(lines) = lines.as_array() {
            for line in lines {
                let group = s_field(line, "name").unwrap_or("快看").to_string();
                let line_id = id_field(line, "id").unwrap_or_default();
                let mut items = Vec::new();
                if let Some(eps) = line.get("episodes").and_then(Value::as_array) {
                    for ep in eps {
                        let title = s_field(ep, "title").unwrap_or("").to_string();
                        let raw = s_field(ep, "url").unwrap_or("").to_string();
                        if title.is_empty() || raw.is_empty() {
                            continue;
                        }
                        let payload = format!("{title}|{vod_id}|{line_id}|{raw}");
                        let token = b64_encode(payload.as_bytes());
                        items.push(Episode { title, url: token });
                    }
                }
                if !items.is_empty() {
                    groups.push(EpisodeGroup { group, items });
                }
            }
        }
    }
    if groups.is_empty() {
        return Err(parse_err(site, "详情没有播放线路"));
    }
    Ok(VideoDetail {
        id: vod_id,
        name,
        cover: s_field(data, "pic").map(str::to_string),
        desc: s_field(data, "content").map(str::to_string),
        year: id_field(data, "year"),
        area: None,
        director: s_field(data, "director").map(str::to_string),
        actor: s_field(data, "actor").map(str::to_string),
        episodes: groups,
    })
}

fn post_analysis(
    cfg: &TvConfig,
    site: &Site,
    keys: &Keys,
    sn: &str,
    url: &str,
    payload: &Value,
) -> Result<Option<String>> {
    let ts = now_ms() / 1000;
    let body = aes_encrypt_b64(&sn[0..16], &sn[16..32], &payload.to_string())
        .ok_or_else(|| parse_err(site, "AES 加密失败"))?;
    let ticket = rsa_private_encrypt_b64(&keys.priv_key, sn.as_bytes())
        .ok_or_else(|| parse_err(site, "RSA 签名失败"))?;
    let client = client_for(&cfg.http)?;
    let mut req = client.post(url).json(&json!({ "body": body, "ticket": ticket }));
    for (k, v) in headers(ts) {
        req = req.header(k, v);
    }
    let resp = req.send().map_err(|e| parse_err(site, format!("分析请求失败: {e}")))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let text = resp.text().map_err(|e| parse_err(site, format!("分析响应读取失败: {e}")))?;
    let outer: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let ticket = outer.get("ticket").and_then(Value::as_str).unwrap_or("");
    let body = outer.get("body").and_then(Value::as_str).unwrap_or("");
    let Some(aes_key) = rsa_public_decrypt(&keys.pub_key, ticket) else {
        return Ok(None);
    };
    if aes_key.len() < 32 {
        return Ok(None);
    }
    let Some(plain) = aes_decrypt_b64(&aes_key[0..16], &aes_key[16..32], body) else {
        return Ok(None);
    };
    let value: Value = match serde_json::from_str(&plain) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let ok = value.get("code").and_then(Value::as_i64) == Some(0)
        && value.get("msg").and_then(Value::as_str) == Some("success");
    if !ok {
        return Ok(None);
    }
    Ok(s_field(&value, "url").map(str::to_string))
}

/// 家族的 `player_content`：base64 四段 → 分析站点轮询 → 直链兜底。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let keys = keys(site)?;
    let raw = b64_decode(id.trim())
        .ok_or_else(|| parse_err(site, "播放 id 不是合法 base64"))?;
    let text = String::from_utf8(raw).map_err(|_| parse_err(site, "播放 id 不是 UTF-8"))?;
    let parts: Vec<&str> = text.split('|').collect();
    if parts.len() < 4 {
        return Err(parse_err(site, "播放 id 分段不足"));
    }
    let (title, vod_id, line_id, url) = (parts[0], parts[1], parts[2], parts[3]);
    let sn = ensure_sn(cfg, site, &keys)?;
    let map = analysis_map(cfg, site, &keys, &sn)?;
    if let Some(candidates) = map.get(line_id) {
        for candidate in candidates {
            let ts = now_ms() / 1000;
            let payload = json!({
                "appId": APP_ID,
                "id": vod_id,
                "title": title,
                "url": url,
                "sign": md5_hex(&format!("{SIGN_PREFIX}{vod_id}{title}{url}{ts}notice")),
                "timestamp": ts,
            });
            if let Some(play) = post_analysis(cfg, site, &keys, &sn, candidate, &payload)? {
                return Ok(json!({ "parse": 0, "playUrl": "", "url": play, "header": "" }));
            }
        }
    }
    if crate::parse::is_direct_media_url(url) {
        return Ok(json!({ "parse": 0, "playUrl": "", "url": url, "header": "" }));
    }
    Err(parse_err(site, format!("未找到可用播放地址（线路 {line_id}）")))
}

/// 家族判定（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 RSA 密钥对（1024 位，仅测试；生产常量见模块头）。
    const TEST_PRIV_B64: &str = "MIICdwIBADANBgkqhkiG9w0BAQEFAASCAmEwggJdAgEAAoGBANBhGPQjXQnvqWhGv4AAOS16v95/t4Jn6U+B4xmYxBEVWVDfyLRy9U8MSfwRva3iXPzsDtREJCbgRXtfIisFFBhmlUWvQ9EfHZn7LGUN5EmWe9SzLv3zDJ9t0yd4wWSimusuIoukT23nGX95hz7dag6FYYs/kAlpcwMKY9vASla5AgMBAAECgYEAmKNnaf41er/Jcg7Q2+xN9diCj3Mycx22s3zLY8aZaIIg6LLkC/phYsP3teMw9BxqTQKFYkt/x9JkUxi+qJCRxHOQpc8lY5pVcTo8U2i4zM4Z6o8g43PdMGGk8mHxHnJfS6zQsM9+BQOFTy9YdRoDDC6V2GHOcuiI9KycT/7ptNkCQQDn6GAQyQElV2YyUX5a2UVOASR49cPOX1MmDKlKLDoNhiDmoGhiuJIKsk94+D8zfNIjRbv+9CsMORXbAj/aMNBzAkEA5gb6zTAAc3s/qCWaYiJ7pQztjCsSNU+6zP/nfJotTgJA6MQoezLxe2Zv6oEsjTm6AxzDqTh+8mRxV29dMNgNIwJAeQwyGJDSqEsWbO1cps8B/dVa1Eh0DCR1oka5lp2/L4/zXG4eDmmI0UyBbwMk+hsNFJwxpMKONbMWVVq3OYyBQQJBANaR2i++wBz48edatEkoW2gZyBjBMKn0Jo8FgxyzAOHMrlmyaQSK/8EZS/pW5oUSPRacJVPbtGdNFID4C9j9IDsCQFucbW3f1LyOMkbxwkxiRzlSGie0vTVu6hR6iv/S88PywNpkE14BnWUH2TJLZ6ttkEUL+fRUoDyGOkBA6E0dwSU=";
    const TEST_PUB_B64: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDQYRj0I10J76loRr+AADkter/ef7eCZ+lPgeMZmMQRFVlQ38i0cvVPDEn8Eb2t4lz87A7URCQm4EV7XyIrBRQYZpVFr0PRHx2Z+yxlDeRJlnvUsy798wyfbdMneMFkoprrLiKLpE9t5xl/eYc+3WoOhWGLP5AJaXMDCmPbwEpWuQIDAQAB";

    fn site(ext: Option<&str>) -> Site {
        Site {
            key: "kuaikan-test".to_string(),
            name: "快看".to_string(),
            kind: 3,
            api: API.to_string(),
            ext: ext.map(|e| json!(e)),
            ..Default::default()
        }
    }

    fn ext_for(base: &str) -> String {
        json!({
            "base": base,
            "um_id": "test-umid",
            "rsa_priv": TEST_PRIV_B64,
            "rsa_pub": TEST_PUB_B64,
        })
        .to_string()
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Kuaikan"));
        assert!(handles("  CSP_KUAIKAN "));
        assert!(!handles("csp_Kuaikan2"));
    }

    #[test]
    fn md5_hex_matches_known_vector() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn aes_round_trip() {
        let enc = aes_encrypt_b64(&LOGIN_KEY[..16], &LOGIN_KEY[16..32], "{\"a\":\"中文\"}")
            .expect("encrypt");
        let dec = aes_decrypt_b64(&LOGIN_KEY[..16], &LOGIN_KEY[16..32], &enc).expect("decrypt");
        assert_eq!(dec, "{\"a\":\"中文\"}");
    }

    #[test]
    fn rsa_private_encrypt_public_decrypt_round_trip() {
        let priv_key = rsa_priv_from_b64(TEST_PRIV_B64).unwrap();
        let pub_key = rsa_pub_from_b64(TEST_PUB_B64).unwrap();
        let enc = rsa_private_encrypt_b64(&priv_key, b"hello-kuaikan").unwrap();
        let dec = rsa_public_decrypt(&pub_key, &enc).unwrap();
        assert_eq!(dec, "hello-kuaikan");
    }

    #[test]
    fn ext_conf_reads_json_hooks() {
        let conf = ext_conf(&site(Some(&ext_for("http://127.0.0.1:1/api/v1"))));
        assert_eq!(conf.base.as_deref(), Some("http://127.0.0.1:1/api/v1"));
        assert_eq!(conf.um_id.as_deref(), Some("test-umid"));
        assert!(conf.rsa_priv.is_some());
        let empty = ext_conf(&site(None));
        assert!(empty.base.is_none() && empty.um_id.is_none());
    }

    #[test]
    fn item_of_parses_fields() {
        let v = json!({ "id": 42, "name": "片名", "pic": "https://i/1.jpg", "progress": "更新至2集" });
        let item = item_of(&v).unwrap();
        assert_eq!(item.id, "42");
        assert_eq!(item.name, "片名");
        assert_eq!(item.cover.as_deref(), Some("https://i/1.jpg"));
        assert_eq!(item.remark.as_deref(), Some("更新至2集"));
    }

    #[test]
    fn player_rejects_illegal_id() {
        let cfg = TvConfig::default();
        let err = player_content(&cfg, &site(None), "", "!!!not-base64!!!").unwrap_err();
        assert!(format!("{err}").contains("base64") || format!("{err}").contains("catvod"));
    }
}

#[cfg(test)]
mod mock_tests {
    use super::*;
    use rsa::RsaPrivateKey;
    use std::thread;
    use tiny_http::{Header, Response, Server};

    const TEST_PRIV_B64: &str = "MIICdwIBADANBgkqhkiG9w0BAQEFAASCAmEwggJdAgEAAoGBANBhGPQjXQnvqWhGv4AAOS16v95/t4Jn6U+B4xmYxBEVWVDfyLRy9U8MSfwRva3iXPzsDtREJCbgRXtfIisFFBhmlUWvQ9EfHZn7LGUN5EmWe9SzLv3zDJ9t0yd4wWSimusuIoukT23nGX95hz7dag6FYYs/kAlpcwMKY9vASla5AgMBAAECgYEAmKNnaf41er/Jcg7Q2+xN9diCj3Mycx22s3zLY8aZaIIg6LLkC/phYsP3teMw9BxqTQKFYkt/x9JkUxi+qJCRxHOQpc8lY5pVcTo8U2i4zM4Z6o8g43PdMGGk8mHxHnJfS6zQsM9+BQOFTy9YdRoDDC6V2GHOcuiI9KycT/7ptNkCQQDn6GAQyQElV2YyUX5a2UVOASR49cPOX1MmDKlKLDoNhiDmoGhiuJIKsk94+D8zfNIjRbv+9CsMORXbAj/aMNBzAkEA5gb6zTAAc3s/qCWaYiJ7pQztjCsSNU+6zP/nfJotTgJA6MQoezLxe2Zv6oEsjTm6AxzDqTh+8mRxV29dMNgNIwJAeQwyGJDSqEsWbO1cps8B/dVa1Eh0DCR1oka5lp2/L4/zXG4eDmmI0UyBbwMk+hsNFJwxpMKONbMWVVq3OYyBQQJBANaR2i++wBz48edatEkoW2gZyBjBMKn0Jo8FgxyzAOHMrlmyaQSK/8EZS/pW5oUSPRacJVPbtGdNFID4C9j9IDsCQFucbW3f1LyOMkbxwkxiRzlSGie0vTVu6hR6iv/S88PywNpkE14BnWUH2TJLZ6ttkEUL+fRUoDyGOkBA6E0dwSU=";
    const TEST_PUB_B64: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDQYRj0I10J76loRr+AADkter/ef7eCZ+lPgeMZmMQRFVlQ38i0cvVPDEn8Eb2t4lz87A7URCQm4EV7XyIrBRQYZpVFr0PRHx2Z+yxlDeRJlnvUsy798wyfbdMneMFkoprrLiKLpE9t5xl/eYc+3WoOhWGLP5AJaXMDCmPbwEpWuQIDAQAB";
    const SN: &str = "0123456789abcdef0123456789abcdef";

    fn envelope(priv_key: &RsaPrivateKey, key: &str, value: &Value) -> Value {
        let body = aes_encrypt_b64(&key[..16], &key[16..32], &value.to_string()).expect("encrypt");
        let ticket = rsa_private_encrypt_b64(priv_key, key.as_bytes()).expect("ticket");
        json!({ "body": body, "ticket": ticket })
    }

    fn respond(request: tiny_http::Request, value: Value) {
        let header =
            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
        let _ = request.respond(Response::from_string(value.to_string()).with_header(header));
    }

    fn start_server() -> (String, thread::JoinHandle<()>) {
        let server = Server::http("127.0.0.1:0").expect("server");
        let root = format!("http://{}", server.server_addr());
        let priv_key = rsa_priv_from_b64(TEST_PRIV_B64).expect("priv");
        let analysis_url = format!("{root}/analysis");
        let sn = SN.to_string();
        let handle = thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let path = request.url().to_string();
                let mut _body = String::new();
                let _ = request.as_reader().read_to_string(&mut _body);
                let value = if path.contains("/user/login.do") {
                    envelope(&priv_key, LOGIN_KEY, &json!({"duration": 3600, "data": sn}))
                } else if path.contains("/user/genres.do") {
                    envelope(
                        &priv_key,
                        &sn,
                        &json!({"genres": [
                            {"typeId": 1, "name": "电影"},
                            {"typeId": 2, "name": "剧集"},
                            {"typeId": 0, "name": "全部"}
                        ]}),
                    )
                } else if path.contains("/search/video.do") {
                    envelope(
                        &priv_key,
                        &sn,
                        &json!({"results": [
                            {"id": 10, "name": "列表片", "pic": "https://i/10.jpg", "progress": "完结"}
                        ]}),
                    )
                } else if path.contains("/search/page.do") {
                    envelope(
                        &priv_key,
                        &sn,
                        &json!({"results": [
                            {"id": 11, "name": "搜索片", "pic": "https://i/11.jpg"}
                        ]}),
                    )
                } else if path.contains("/video/play/") {
                    let playback = json!([{
                        "name": "线路一",
                        "id": 1,
                        "episodes": [
                            {"title": "第1集", "url": "https://cdn.test/1.m3u8"},
                            {"title": "第2集", "url": "https://cdn.test/2.m3u8"}
                        ]
                    }])
                    .to_string();
                    let enc = aes_encrypt_b64(NEWPLAY_KEY, NEWPLAY_IV, &playback).expect("np");
                    envelope(
                        &priv_key,
                        &sn,
                        &json!({"data": {
                            "id": "17", "name": "详情片", "pic": "https://i/17.jpg",
                            "year": "2024", "progress": "更新至2集", "actor": "演员",
                            "director": "导演", "content": "简介", "newPlayback": enc
                        }}),
                    )
                } else if path.contains("/user/config.do") {
                    envelope(
                        &priv_key,
                        &sn,
                        &json!({"data": {"newAnalysis": [
                            {"id": "1", "analyses": [{"url": analysis_url}]}
                        ]}}),
                    )
                } else if path.contains("/analysis") {
                    envelope(
                        &priv_key,
                        &sn,
                        &json!({"code": 0, "msg": "success", "url": "https://cdn.test/analyzed.m3u8"}),
                    )
                } else {
                    json!({"error": path})
                };
                respond(request, value);
            }
        });
        (root, handle)
    }

    fn site_for(base: &str) -> Site {
        Site {
            key: "kuaikan-mock".to_string(),
            name: "快看".to_string(),
            kind: 3,
            api: API.to_string(),
            ext: Some(json!(json!({
                "base": base,
                "um_id": "mock-umid",
                "rsa_priv": TEST_PRIV_B64,
                "rsa_pub": TEST_PUB_B64,
            })
            .to_string())),
            ..Default::default()
        }
    }

    #[test]
    fn mock_end_to_end() {
        let (root, _handle) = start_server();
        let site = site_for(&format!("{root}/api/v1"));
        let cfg = TvConfig::default();

        let cats = categories(&cfg, &site).expect("categories");
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "电影");

        let list = videos(&cfg, &site, "1", 1).expect("videos");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "10");
        assert_eq!(list[0].name, "列表片");

        let found = search(&cfg, &site, "片").expect("search");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "搜索片");

        let d = detail(&cfg, &site, "17").expect("detail");
        assert_eq!(d.name, "详情片");
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].items.len(), 2);
        let ep = &d.episodes[0].items[0];
        assert_eq!(ep.title, "第1集");
        let raw = b64_decode(&ep.url).expect("token b64");
        assert_eq!(String::from_utf8(raw).expect("utf8"), "第1集|17|1|https://cdn.test/1.m3u8");

        let play = player_content(&cfg, &site, "", &ep.url).expect("play");
        assert_eq!(play["parse"], 0);
        assert_eq!(play["url"], "https://cdn.test/analyzed.m3u8");

        let direct = b64_encode("第9集|17|999|https://cdn.test/direct.mp4".as_bytes());
        let play2 = player_content(&cfg, &site, "", &direct).expect("direct");
        assert_eq!(play2["url"], "https://cdn.test/direct.mp4");
    }
}
