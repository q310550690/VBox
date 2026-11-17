//! HLS 加密分片：播放列表解析 + AES-128-CBC 解密
//!
//! 供两处共用：
//! - `hls_proxy`：边看边存导出时把缓存里的密文分片解密后合并；
//! - `download`：下载引擎对 AES-128 HLS 分片做离线解密后合并。
//!
//! 规则（RFC 8216 §4.3.2.4）：
//! - `#EXT-X-KEY:METHOD=AES-128,URI=...,IV=0x...` 对**后续分片**生效，可多次切换；
//! - 没写 `IV` 时，IV = 该分片的 media sequence number（16 字节大端）；
//! - 每个分片是独立的 AES-128-CBC 密文（通常带 PKCS#7 padding）；
//! - `METHOD=NONE` 关闭加密；`SAMPLE-AES` 等暂不支持。
//!
//! `N_m3u8DL-RE` 之类的工具不能参与这里（我们只需要离线解密，不需要下载器）。

use std::path::Path;

use aes::Aes128;
use cbc::Decryptor;
use cipher::block_padding::NoPadding;
use cipher::{BlockDecryptMut, KeyIvInit};

/// 分片加密方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMethod {
    /// 整段 AES-128-CBC（本地可解）
    Aes128,
    /// Apple SAMPLE-AES：只加密 NAL / 音频帧的一部分，需按样本级解密
    SampleAes,
}

/// 一个分片引用的密钥：地址 + 可选显式 IV（不写则用 sequence number）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRef {
    /// 密钥文件绝对地址
    pub url: String,
    /// 显式 IV（`IV=0x...`）
    pub iv: Option<[u8; 16]>,
    /// 加密方式
    pub method: KeyMethod,
}

/// 解析出的媒体播放列表加密信息（顺序与分片一一对应）。
#[derive(Debug, Clone, Default)]
pub struct MediaCrypto {
    /// `#EXT-X-MAP` 初始化段（绝对地址，一般未加密）
    pub init: Option<String>,
    /// 分片绝对地址，按播放顺序
    pub segments: Vec<String>,
    /// 每个分片的密钥（None = 明文分片），与 `segments` 对齐
    pub keys: Vec<Option<KeyRef>>,
    /// 每个分片的 `#EXT-X-BYTERANGE`（长度, 起始偏移），与 `segments` 对齐
    pub ranges: Vec<Option<(u64, u64)>>,
    /// 每个分片的时长（`#EXTINF`，秒；缺省 0），与 `segments` 对齐
    pub durations: Vec<f64>,
    /// `#EXT-X-MEDIA-SEQUENCE`（缺省 0）
    pub media_sequence: u64,
    /// 不支持的加密方式（含 DRM / SAMPLE-AES-CTR 等），用于直接报错
    pub unsupported: Option<String>,
    /// 是否含 SAMPLE-AES（可交给 ffmpeg 按样本级解密）
    pub sample_aes: bool,
    /// 是否含 `#EXT-X-BYTERANGE`（调用方自行决定是否支持）
    pub has_byterange: bool,
}

impl MediaCrypto {
    /// 是否有任一分片需要解密。
    pub fn encrypted(&self) -> bool {
        self.keys.iter().any(Option::is_some)
    }

    /// 是否全部加密分片都是整段 AES-128（可用本地解密）。
    pub fn only_aes128(&self) -> bool {
        self.keys
            .iter()
            .flatten()
            .all(|key| key.method == KeyMethod::Aes128)
    }
}

/// 解析媒体播放列表的加密信息（`playlist_url` 用于解析相对地址）。
pub fn parse_media_playlist(text: &str, playlist_url: &str) -> MediaCrypto {
    let mut out = MediaCrypto::default();
    let mut current: Option<KeyRef> = None;
    let mut pending_duration: Option<f64> = None;
    let mut pending_range: Option<(u64, Option<u64>)> = None;
    // BYTERANGE 省略 offset 时，紧接同一 URI 上一段的末尾
    let mut range_ends: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            out.media_sequence = rest.trim().parse::<u64>().unwrap_or(0);
            continue;
        }
        if line.starts_with("#EXT-X-KEY") || line.starts_with("#EXT-X-SESSION-KEY") {
            let attrs = parse_attrs(line);
            let method = attrs
                .get("METHOD")
                .map(|value| value.trim().to_ascii_uppercase())
                .unwrap_or_default();
            if method == "NONE" {
                current = None;
            } else if method == "AES-128" {
                if let Some(uri) = attrs.get("URI") {
                    let iv = attrs.get("IV").and_then(|value| parse_hex_iv(value));
                    current = Some(KeyRef {
                        url: resolve_url(playlist_url, uri),
                        iv,
                        method: KeyMethod::Aes128,
                    });
                }
            } else if method == "SAMPLE-AES" {
                // KEYFORMAT 非 identity（如 com.apple.streamingkeydelivery / urn:uuid…）
                // 或 skd:// 都是 DRM，无法在没有授权密钥的情况下解密。
                let keyformat = attrs
                    .get("KEYFORMAT")
                    .map(String::as_str)
                    .unwrap_or("identity");
                let uri = attrs.get("URI").map(String::as_str).unwrap_or("");
                if !keyformat.eq_ignore_ascii_case("identity") || uri.starts_with("skd://") {
                    out.unsupported = Some(format!(
                        "SAMPLE-AES + DRM（KEYFORMAT={keyformat}），需要 DRM 授权密钥，无法离线解密"
                    ));
                    current = None;
                } else if let Some(uri) = attrs.get("URI") {
                    out.sample_aes = true;
                    let iv = attrs.get("IV").and_then(|value| parse_hex_iv(value));
                    current = Some(KeyRef {
                        url: resolve_url(playlist_url, uri),
                        iv,
                        method: KeyMethod::SampleAes,
                    });
                }
            } else if !method.is_empty() {
                out.unsupported = Some(method);
                current = None;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let duration = rest
                .split(',')
                .next()
                .and_then(|value| value.trim().parse::<f64>().ok())
                .unwrap_or(0.0);
            pending_duration = Some(duration);
            continue;
        }
        if line.starts_with("#EXT-X-MAP") {
            if let Some(uri) = parse_attrs(line).get("URI") {
                out.init = Some(resolve_url(playlist_url, uri));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            out.has_byterange = true;
            let (length, offset) = match rest.trim().split_once('@') {
                Some((length, offset)) => (
                    length.trim().parse::<u64>().ok(),
                    offset.trim().parse::<u64>().ok(),
                ),
                None => (rest.trim().parse::<u64>().ok(), None),
            };
            if let Some(length) = length {
                pending_range = Some((length, offset));
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let absolute = resolve_url(playlist_url, line);
        let range = pending_range.take().map(|(length, offset)| {
            let start = offset.unwrap_or_else(|| range_ends.get(&absolute).copied().unwrap_or(0));
            range_ends.insert(absolute.clone(), start.saturating_add(length));
            (length, start)
        });
        out.segments.push(absolute);
        out.keys.push(current.clone());
        out.ranges.push(range);
        out.durations.push(pending_duration.take().unwrap_or(0.0));
    }
    out
}

/// 解析 `KEY=VALUE` 形式标签属性（引号可选）。同名取第一个。
fn parse_attrs(line: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Some(colon) = line.find(':') else {
        return map;
    };
    for part in line[colon + 1..].split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        map.entry(key.to_string())
            .or_insert_with(|| value.to_string());
    }
    map
}

/// 相对地址解析（绝对地址原样返回）。
pub fn resolve_url(base: &str, value: &str) -> String {
    let value = value.trim();
    if value.starts_with("http://") || value.starts_with("https://") || value.starts_with("data:") {
        return value.to_string();
    }
    url::Url::parse(base)
        .ok()
        .and_then(|base| base.join(value).ok())
        .map(|url| url.to_string())
        .unwrap_or_else(|| value.to_string())
}

/// 解析 `IV=0x...`（短于 32 个 hex 字符时左补 0）。
pub fn parse_hex_iv(text: &str) -> Option<[u8; 16]> {
    let trimmed = text.trim();
    let hex_text = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    if hex_text.is_empty()
        || hex_text.len() > 32
        || !hex_text.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    let padded = format!("{hex_text:0>32}");
    let bytes = hex::decode(padded).ok()?;
    bytes.try_into().ok()
}

/// 分片序号 → IV（16 字节大端；高位补 0）。
pub fn sequence_iv(sequence: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&sequence.to_be_bytes());
    iv
}

/// 密钥文件字节 → 16 字节密钥。
///
/// 兼容三种来源：纯 16 字节二进制、带换行的二进制、以及少数站点给的 32 字符 hex 文本。
pub fn key_bytes(raw: &[u8]) -> Result<[u8; 16], String> {
    let trimmed = raw.strip_suffix(b"\n").unwrap_or(raw);
    let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
    if trimmed.len() == 32
        && trimmed.iter().all(u8::is_ascii_hexdigit)
        && let Ok(text) = std::str::from_utf8(trimmed)
        && let Ok(bytes) = hex::decode(text)
    {
        return bytes
            .try_into()
            .map_err(|_| "HLS 密钥长度不是 16 字节".to_string());
    }
    if trimmed.len() < 16 {
        return Err(format!("HLS 密钥长度不足（{} 字节）", trimmed.len()));
    }
    Ok(trimmed[..16].try_into().expect("长度已校验"))
}

/// AES-128-CBC 解密单个分片。
///
/// 先按规范去 PKCS#7 padding；个别源没有按规范补 padding（长度为 16 的整数倍）时，
/// 退化为按块解密、保留全部明文。
pub fn decrypt_segment(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if !data.len().is_multiple_of(16) {
        return Err(format!(
            "AES-128 密文长度不是 16 的整数倍（{} 字节）",
            data.len()
        ));
    }
    let mut buffer = data.to_vec();
    let decryptor = Decryptor::<Aes128>::new(key.into(), iv.into());
    match decryptor.decrypt_padded_mut::<cipher::block_padding::Pkcs7>(&mut buffer) {
        Ok(plain) => Ok(plain.to_vec()),
        Err(_) => {
            let mut buffer = data.to_vec();
            let decryptor = Decryptor::<Aes128>::new(key.into(), iv.into());
            let plain = decryptor
                .decrypt_padded_mut::<NoPadding>(&mut buffer)
                .map_err(|e| format!("AES-128 解密失败: {e}"))?;
            Ok(plain.to_vec())
        }
    }
}

/// 读密钥文件并解密一个分片文件（`iv` 缺省时由调用方用 sequence 计算）。
pub fn decrypt_file(
    segment_path: &Path,
    key_path: &Path,
    iv: &[u8; 16],
) -> Result<Vec<u8>, String> {
    let raw = std::fs::read(segment_path)
        .map_err(|e| format!("读取分片失败({}): {e}", segment_path.display()))?;
    let key_raw = std::fs::read(key_path)
        .map_err(|e| format!("读取密钥失败({}): {e}", key_path.display()))?;
    let key = key_bytes(&key_raw)?;
    decrypt_segment(&raw, &key, iv)
}

/// 单元测试用：AES-128-CBC/PKCS#7 加密（仅测试）。
#[cfg(test)]
pub fn encrypt_for_test(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
    use cbc::Encryptor;
    use cipher::{BlockEncryptMut, block_padding::Pkcs7};
    let mut buffer = data.to_vec();
    let encryptor = Encryptor::<Aes128>::new(key.into(), iv.into());
    let length = buffer.len();
    let padding = 16 - (length % 16);
    buffer.resize(length + padding, 0);
    encryptor
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, length)
        .expect("测试加密失败")
        .to_vec()
}

/* ---------------- SAMPLE-AES：借助 ffmpeg 按样本级解密 ---------------- */

/// 用户配置的 ffmpeg 路径（设置页写入）。`ffmpeg_prefs` 在启动/保存/一键集成后
/// 会把「用户指定 → 已下载集成 → 随包集成」的解析结果写到这里，优先于环境变量与 PATH。
static FFMPEG_OVERRIDE: std::sync::OnceLock<std::sync::RwLock<Option<String>>> =
    std::sync::OnceLock::new();

/// 设置 / 清除用户配置的 ffmpeg 路径（空串按清除处理）。
pub fn set_ffmpeg_override(path: Option<String>) {
    let slot = FFMPEG_OVERRIDE.get_or_init(|| std::sync::RwLock::new(None));
    let value = path
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Ok(mut guard) = slot.write() {
        *guard = value;
    }
}

/// ffmpeg 可执行文件：设置页解析结果（用户指定 / 已下载集成 / 随包集成）→
/// `VBOX_FFMPEG` → PATH 里的 `ffmpeg`。
pub fn ffmpeg_binary() -> String {
    if let Some(slot) = FFMPEG_OVERRIDE.get()
        && let Ok(guard) = slot.read()
        && let Some(path) = guard.as_ref()
    {
        return path.clone();
    }
    std::env::var("VBOX_FFMPEG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ffmpeg".to_string())
}

/// 用 ffmpeg 读一个 HLS 播放列表（本地路径或本地代理 URL）并输出解密后的 TS。
///
/// 适用于 SAMPLE-AES：按样本级解密是 ffmpeg（libavformat）自带能力，
/// 在 Rust 侧重新实现 TS/PES/NAL 解析得不偿失且容易出错。
/// `-c copy` 只解密/重封装，不重编码。
pub fn run_ffmpeg_hls_to_ts(
    input: &str,
    out: &Path,
    timeout: std::time::Duration,
) -> Result<(), String> {
    run_ffmpeg(
        vec![
            "-i".to_string(),
            input.to_string(),
            "-map".to_string(),
            "0".to_string(),
            "-c".to_string(),
            "copy".to_string(),
            "-f".to_string(),
            "mpegts".to_string(),
        ],
        out,
        timeout,
        true,
    )
}

/// 分离音轨合并：视频播放列表 + 音频播放列表 → 单个 TS（`-c copy`，不重编码）。
pub fn run_ffmpeg_merge_to_ts(
    video: &str,
    audio: &str,
    out: &Path,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let allow_hls_extensions = video.to_ascii_lowercase().contains(".m3u8")
        || video.contains("/hls/")
        || audio.to_ascii_lowercase().contains(".m3u8")
        || audio.contains("/hls/");
    run_ffmpeg(
        vec![
            "-i".to_string(),
            video.to_string(),
            "-i".to_string(),
            audio.to_string(),
            "-map".to_string(),
            "0:v:0".to_string(),
            "-map".to_string(),
            "1:a:0".to_string(),
            "-c".to_string(),
            "copy".to_string(),
            "-f".to_string(),
            "mpegts".to_string(),
        ],
        out,
        timeout,
        allow_hls_extensions,
    )
}

/// 通用兜底：让 ffmpeg 直接读播放地址并重封装（DASH / 非 .m3u8 HLS / FLV 等）。
///
/// 输出容器按扩展名决定（`.mp4` → mp4，其余 → mpegts）；`-c copy` 不重编码。
pub fn run_ffmpeg_remux(
    input: &str,
    out: &Path,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let format = if out
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
    {
        "mp4"
    } else {
        "mpegts"
    };
    let allow_hls_extensions =
        input.to_ascii_lowercase().contains(".m3u8") || input.contains("/hls/");
    run_ffmpeg(
        vec![
            "-i".to_string(),
            input.to_string(),
            "-map".to_string(),
            "0".to_string(),
            "-c".to_string(),
            "copy".to_string(),
            "-f".to_string(),
            format.to_string(),
        ],
        out,
        timeout,
        allow_hls_extensions,
    )
}

/// ffmpeg 通用执行：参数里已包含输入/输出格式，这里只补日志、超时与窗口隐藏。
fn run_ffmpeg(
    args: Vec<String>,
    out: &Path,
    timeout: std::time::Duration,
    allow_hls_extensions: bool,
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let bin = ffmpeg_binary();
    let log_path = out.with_extension("ffmpeg.log");
    let log = std::fs::File::create(&log_path).map_err(|e| format!("创建 ffmpeg 日志失败: {e}"))?;
    let log_err = log
        .try_clone()
        .map_err(|e| format!("创建 ffmpeg 日志失败: {e}"))?;

    let mut command = Command::new(&bin);
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y");
    // 只对 HLS 输入加：其他 demuxer 不认识这个选项会直接报错
    if allow_hls_extensions {
        command.arg("-allowed_extensions").arg("ALL");
    }
    command
        .args(&args)
        .arg(out)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = std::fs::remove_file(&log_path);
            return Err(format!(
                "未找到 ffmpeg（当前格式的离线保存需要）：{err}。请在「设置 → 外部工具 · FFmpeg」点「一键下载并集成」，或安装 ffmpeg / 指定可执行文件（也可设 VBOX_FFMPEG）"
            ));
        }
    };

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let tail = read_log_tail(&log_path);
                    let _ = std::fs::remove_file(&log_path);
                    return Err(format!("ffmpeg 处理超时（{tail}）"));
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = std::fs::remove_file(&log_path);
                return Err(format!("等待 ffmpeg 失败: {err}"));
            }
        }
    };

    let tail = read_log_tail(&log_path);
    let _ = std::fs::remove_file(&log_path);
    if !status.success() {
        return Err(format!(
            "ffmpeg 处理失败（exit={:?}）：{tail}",
            status.code()
        ));
    }
    let size = std::fs::metadata(out).map(|meta| meta.len()).unwrap_or(0);
    if size == 0 {
        return Err(format!("ffmpeg 输出为空：{tail}"));
    }
    Ok(())
}

fn read_log_tail(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    text.lines()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ")
}

/// 为下载引擎生成「本地分片 + SAMPLE-AES 密钥」的临时播放列表（交给 ffmpeg 解密）。
pub fn build_local_playlist(
    crypto: &MediaCrypto,
    segment_files: &[String],
    key_files: &std::collections::HashMap<String, String>,
    media_sequence: u64,
) -> Result<String, String> {
    if crypto.segments.len() != segment_files.len() {
        return Err("分片数量与播放列表不一致".to_string());
    }
    let mut out = String::from("#EXTM3U\n#EXT-X-VERSION:3\n");
    out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{media_sequence}\n"));
    out.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    for (index, name) in segment_files.iter().enumerate() {
        if let Some(key_ref) = crypto.keys.get(index).and_then(|slot| slot.as_ref()) {
            let key_file = key_files
                .get(&key_ref.url)
                .ok_or_else(|| format!("缺少密钥文件: {}", key_ref.url))?;
            let iv = key_ref
                .iv
                .unwrap_or_else(|| sequence_iv(media_sequence + index as u64));
            out.push_str(&format!(
                "#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"{key_file}\",IV=0x{}\n",
                hex::encode(iv)
            ));
        }
        let duration = crypto.durations.get(index).copied().unwrap_or(0.0);
        out.push_str(&format!("#EXTINF:{duration:.3},\n{name}\n"));
    }
    out.push_str("#EXT-X-ENDLIST\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYLIST: &str = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x000102030405060708090a0b0c0d0e0f\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2.0,\nseg1.m4s\n#EXTINF:2.0,\nseg2.m4s\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:2.0,\nseg3.ts\n#EXT-X-ENDLIST\n";

    #[test]
    fn parses_keys_iv_sequence_and_clear_switch() {
        let parsed = parse_media_playlist(PLAYLIST, "https://cdn.example.com/a/v.m3u8");
        assert_eq!(
            parsed.init.as_deref(),
            Some("https://cdn.example.com/a/init.mp4")
        );
        assert_eq!(parsed.segments.len(), 3);
        assert_eq!(parsed.keys.len(), 3);
        assert_eq!(parsed.media_sequence, 7);
        let key = parsed.keys[0].as_ref().expect("第一段应加密");
        assert_eq!(key.url, "https://cdn.example.com/a/key.bin");
        assert_eq!(
            key.iv,
            Some([
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e, 0x0f
            ])
        );
        assert!(parsed.keys[2].is_none(), "METHOD=NONE 后应回到明文");
        assert!(parsed.encrypted());
        assert!(parsed.unsupported.is_none());
    }

    #[test]
    fn flags_drm_sample_aes_as_unsupported() {
        // skd:// = FairPlay，没有授权密钥无法解密
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\"\n#EXTINF:2,\nseg1.ts\n";
        let parsed = parse_media_playlist(text, "https://x/y.m3u8");
        assert!(
            parsed.unsupported.as_deref().unwrap_or("").contains("DRM"),
            "{:?}",
            parsed.unsupported
        );
        assert!(!parsed.sample_aes);
    }

    #[test]
    fn flags_non_identity_keyformat_as_unsupported() {
        // FairPlay / Widevine 的 KEYFORMAT 不是 identity，同样没有授权密钥
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,KEYFORMAT=\"com.apple.streamingkeydelivery\",URI=\"key.bin\"\n#EXTINF:2,\nseg1.ts\n";
        let parsed = parse_media_playlist(text, "https://x/y.m3u8");
        assert!(
            parsed
                .unsupported
                .as_deref()
                .unwrap_or("")
                .contains("KEYFORMAT=com.apple.streamingkeydelivery"),
            "{:?}",
            parsed.unsupported
        );
        assert!(!parsed.sample_aes);
    }

    #[test]
    fn accepts_identity_sample_aes() {
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key.bin\",KEYFORMAT=\"identity\"\n#EXTINF:2,\nseg1.ts\n";
        let parsed = parse_media_playlist(text, "https://x/y.m3u8");
        assert!(parsed.sample_aes, "{:?}", parsed.unsupported);
        assert!(parsed.unsupported.is_none());
        assert_eq!(
            parsed.keys[0].as_ref().unwrap().method,
            KeyMethod::SampleAes
        );
        assert_eq!(parsed.durations, vec![2.0]);
    }

    #[test]
    fn builds_local_sample_aes_playlist() {
        let text = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:3\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key.bin\"\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nseg2.ts\n#EXT-X-ENDLIST\n";
        let parsed = parse_media_playlist(text, "https://cdn.example.com/a/v.m3u8");
        let mut key_files = std::collections::HashMap::new();
        key_files.insert(
            "https://cdn.example.com/a/key.bin".to_string(),
            "key_0001.bin".to_string(),
        );
        let playlist = build_local_playlist(
            &parsed,
            &["seg_00001.ts".to_string(), "seg_00002.ts".to_string()],
            &key_files,
            3,
        )
        .expect("生成失败");
        assert!(
            playlist.contains("#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key_0001.bin\""),
            "{playlist}"
        );
        assert!(playlist.contains("#EXTINF:4.000,"), "{playlist}");
        assert!(playlist.contains("seg_00001.ts"));
        assert!(playlist.contains("seg_00002.ts"));
        // IV 按 media_sequence 递增：第 1 段 = 3，第 2 段 = 4
        assert!(
            playlist.contains("IV=0x00000000000000000000000000000003"),
            "{playlist}"
        );
        assert!(
            playlist.contains("IV=0x00000000000000000000000000000004"),
            "{playlist}"
        );
    }

    #[test]
    fn parses_byterange_offsets() {
        let text = "#EXTM3U\n#EXT-X-VERSION:4\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nall.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10\nall.ts\n#EXT-X-ENDLIST\n";
        let parsed = parse_media_playlist(text, "https://cdn.example.com/a/v.m3u8");
        assert!(parsed.has_byterange);
        assert_eq!(parsed.segments.len(), 2);
        assert_eq!(parsed.ranges, vec![Some((10, 0)), Some((10, 10))]);
        // 两个分片指向同一个远端文件（导出时只下载一次、按范围切片）
        assert_eq!(parsed.segments[0], parsed.segments[1]);
    }

    #[test]
    fn iv_helpers() {
        assert_eq!(sequence_iv(0), [0u8; 16]);
        assert_eq!(sequence_iv(1)[15], 1);
        let short = parse_hex_iv("0x1").expect("短 IV 应左补 0");
        assert_eq!(short[15], 1);
        assert!(parse_hex_iv("00").is_some());
        assert!(parse_hex_iv("zz").is_none());
    }

    #[test]
    fn decrypts_pkcs7_and_unpadded_segments() {
        let key = [7u8; 16];
        let iv = sequence_iv(42);
        // 规范实现：带 PKCS7 padding
        let cipher = encrypt_for_test(b"hello world", &key, &iv);
        assert_eq!(decrypt_segment(&cipher, &key, &iv).unwrap(), b"hello world");
        // 非规范：恰好 16 字节且无 padding
        let cipher = encrypt_for_test(b"0123456789abcdef", &key, &iv);
        let plain = decrypt_segment(&cipher, &key, &iv).unwrap();
        assert_eq!(plain, b"0123456789abcdef");
    }

    /// ffmpeg 调用链路自检：生成一个测试 TS 分片，走「本地播放列表 → ffmpeg 重封装」。
    ///
    /// 没装 ffmpeg 时跳过（SAMPLE-AES 解密本身依赖它）；有则验证命令/超时/输出路径正确。
    #[test]
    fn ffmpeg_remux_plumbing() {
        use std::process::Command;
        let bin = ffmpeg_binary();
        let available = Command::new(&bin)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !available {
            eprintln!("skip: 未安装 ffmpeg");
            return;
        }

        let dir = std::env::temp_dir().join(format!("vbox-ffmpeg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        let segment = dir.join("seg1.ts");
        let made = Command::new(&bin)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=128x72:rate=10",
                "-c:v",
                "mpeg2video",
                "-f",
                "mpegts",
            ])
            .arg(&segment)
            .status()
            .expect("生成测试分片失败");
        assert!(made.success(), "ffmpeg 生成测试分片失败");

        let playlist = dir.join("index.m3u8");
        std::fs::write(
            &playlist,
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nseg1.ts\n#EXT-X-ENDLIST\n",
        )
        .expect("写播放列表失败");

        let out = dir.join("merged.ts");
        run_ffmpeg_hls_to_ts(
            &playlist.to_string_lossy(),
            &out,
            std::time::Duration::from_secs(60),
        )
        .expect("ffmpeg 重封装失败");
        let size = std::fs::metadata(&out).map(|meta| meta.len()).unwrap_or(0);
        assert!(size > 1024, "输出过小: {size}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// DASH 兜底自检：生成一个本地 .mpd，经 `run_ffmpeg_remux` 重封装为 TS。
    #[test]
    fn ffmpeg_dash_remux_plumbing() {
        use std::process::Command;
        let bin = ffmpeg_binary();
        let available = Command::new(&bin)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !available {
            eprintln!("skip: 未安装 ffmpeg");
            return;
        }

        let dir = std::env::temp_dir().join(format!("vbox-dash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        let manifest = dir.join("manifest.mpd");
        let made = Command::new(&bin)
            .current_dir(&dir)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=128x72:rate=10",
                "-c:v",
                "mpeg2video",
                "-f",
                "dash",
            ])
            .arg(&manifest)
            .status()
            .expect("生成测试 DASH 失败");
        if !made.success() {
            eprintln!("skip: 当前 ffmpeg 不带 dash muxer");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let out = dir.join("merged.ts");
        run_ffmpeg_remux(
            &manifest.to_string_lossy(),
            &out,
            std::time::Duration::from_secs(60),
        )
        .expect("DASH 兜底重封装失败");
        let size = std::fs::metadata(&out).map(|meta| meta.len()).unwrap_or(0);
        assert!(size > 1024, "输出过小: {size}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 分离音轨合并自检：分别生成视频/音频 TS，双输入合并为一个 TS。
    #[test]
    fn ffmpeg_merge_video_audio_plumbing() {
        use std::process::Command;
        let bin = ffmpeg_binary();
        let available = Command::new(&bin)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !available {
            eprintln!("skip: 未安装 ffmpeg");
            return;
        }

        let dir = std::env::temp_dir().join(format!("vbox-merge-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        let video = dir.join("video.ts");
        let audio = dir.join("audio.ts");
        let make = |args: &[&str], out: &Path| {
            Command::new(&bin)
                .args(args)
                .arg(out)
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        };
        assert!(
            make(
                &[
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=duration=1:size=128x72:rate=10",
                    "-c:v",
                    "mpeg2video",
                    "-f",
                    "mpegts"
                ],
                &video
            ),
            "生成视频分片失败"
        );
        assert!(
            make(
                &[
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=1",
                    "-c:a",
                    "aac",
                    "-f",
                    "mpegts"
                ],
                &audio
            ),
            "生成音频分片失败"
        );

        let out = dir.join("merged.ts");
        run_ffmpeg_merge_to_ts(
            &video.to_string_lossy(),
            &audio.to_string_lossy(),
            &out,
            std::time::Duration::from_secs(60),
        )
        .expect("音视频合并失败");
        let size = std::fs::metadata(&out).map(|meta| meta.len()).unwrap_or(0);
        assert!(size > 1024, "输出过小: {size}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_bytes_variants() {
        let binary = [3u8; 16];
        assert_eq!(key_bytes(&binary).unwrap(), binary);
        let with_newline = [4u8; 17];
        assert_eq!(key_bytes(&with_newline).unwrap(), [4u8; 16]);
        let hex_text = b"000102030405060708090a0b0c0d0e0f";
        assert_eq!(
            key_bytes(hex_text).unwrap(),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
        assert!(key_bytes(&[1u8; 8]).is_err());
    }
}
