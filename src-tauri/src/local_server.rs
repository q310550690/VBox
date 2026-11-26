//! 本地 HTTP 服务（对应 TVBox 的 9978 `ControlManager` / `RemoteServer`）
//!
//! 只监听回环地址，端口由系统分配（`127.0.0.1:0`），启动后把 `base_url` 写进
//! `AppState.local_base`，供 `clan://` 配置/资源以及播放器代理使用。
//!
//! 当前实现三个端点（与 TVBox 对齐的最小集合）：
//! - `GET /health`：存活探针，返回 `ok`。
//! - `GET /file/{path}`：只读文件（对应 TVBox 的 `/file/` 路由），支持
//!   `clan://localhost/...` 映射到本地目录；也可显式传绝对路径。
//! - `GET /proxy?do=live&type=txt&ext=<base64>`：直播订阅代理。`ext` 为
//!   Base64（URL_SAFE | NO_WRAP）编码的真实 URL，或直接是 http(s) URL；
//!   服务端代为拉取并返回 `text/plain`（绕开跨域/防盗链，TVBox 同款）。
//!
//! 安全边界：
//! - 仅绑定 `127.0.0.1`，不对局域网开放；
//! - `/file` 拒绝任何含 `..` 的路径；相对路径 canonicalize 后必须仍在 root 内，
//!   且只允许普通文件；
//! - 单次响应上限 [`MAX_RESPONSE_BYTES`]（10MB）；
//! - 不做 HTTPS MITM / 不安装证书。

use base64::Engine as _;
use percent_encoding::percent_decode_str;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::hls_proxy::HlsProxy;

/// 单次响应体上限：10MB（超过时返回 413/502，避免大文件拖垮桌面端）。
pub const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// 图片代理请求头：豆瓣图床直连会 418，必须带 Referer。
const IMG_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
const IMG_REFERER: &str = "https://movie.douban.com/";
/// 图片代理超时 10 秒。
const IMG_TIMEOUT: Duration = Duration::from_secs(10);
/// 图片代理缓存 1 天。
const IMG_CACHE_CONTROL: &str = "public, max-age=86400";

/// 本地服务句柄。`base_url` 形如 `http://127.0.0.1:34567`（无结尾斜杠），
/// 与 `vbox_core::config::LoadOptions.local_base` 的约定一致：
/// `clan://localhost/x.json` → `{base_url}/file/x.json`。
#[allow(dead_code)] // base_url 已被 lib.rs 读取；port/handle/stop 预留给优雅停机与排查
pub struct LocalServer {
    pub base_url: String,
    pub port: u16,
    pub handle: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

impl LocalServer {
    /// 停止接收新请求并等待监听线程退出（测试/退出时调用；不调用则随进程结束）。
    #[allow(dead_code)]
    pub fn shutdown(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.handle.join();
    }
}

/// 启动本地服务。
///
/// `root` 为 `clan://localhost/...` 的文件根目录（通常是
/// `<app_data_dir>/clan`），不存在时自动创建。
/// `hls` 为 HLS 边下边播代理（`/hls/...` 路由），会把本地端口回填给它。
pub fn start(root: PathBuf, hls: Arc<HlsProxy>) -> Result<LocalServer, String> {
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("创建 clan 目录失败 {}: {e}", root.display()))?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("解析 clan 目录失败 {}: {e}", root.display()))?;
    let root = Arc::new(root);

    let server = Server::http(("127.0.0.1", 0)).map_err(|e| format!("监听 127.0.0.1 失败: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .ok_or_else(|| "无法获取本地服务端口".to_string())?;
    let base_url = format!("http://127.0.0.1:{port}");
    hls.set_base_url(base_url.clone());

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_root = root.clone();
    let thread_hls = Arc::clone(&hls);
    let handle = std::thread::spawn(move || {
        while !thread_stop.load(Ordering::SeqCst) {
            match server.recv_timeout(Duration::from_millis(200)) {
                Ok(Some(request)) => {
                    // 每个请求一个线程：/proxy 可能等待上游，避免阻塞 /file 与 /health。
                    let root = thread_root.clone();
                    let hls = Arc::clone(&thread_hls);
                    std::thread::spawn(move || {
                        if let Err(err) = handle_request(request, &root, &hls) {
                            eprintln!("[vbox-local] 请求处理失败: {err}");
                        }
                    });
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });

    Ok(LocalServer {
        base_url,
        port,
        handle,
        stop,
    })
}

/* ---------------- 路由 ---------------- */

fn handle_request(request: Request, root: &Path, hls: &Arc<HlsProxy>) -> Result<(), String> {
    if request.method() != &Method::Get {
        return respond_text(
            request,
            405,
            "method not allowed",
            "text/plain; charset=utf-8",
        );
    }

    let (raw_path, query) = split_url(request.url());
    let path = match percent_decode_str(&raw_path).decode_utf8() {
        Ok(path) => path.into_owned(),
        Err(_) => {
            return respond_text(
                request,
                400,
                "path is not valid UTF-8",
                "text/plain; charset=utf-8",
            );
        }
    };

    // 注意：路径只在此处 decode 一次，避免 %252e%252e 之类的二次解码绕过。
    match path.as_str() {
        "/health" | "/healthz" => respond_text(request, 200, "ok", "text/plain; charset=utf-8"),
        "/img" => handle_img(request, &query),
        "/proxy" => handle_proxy(request, &query),
        _ if HlsProxy::matches(&path) => hls.handle(request, &path),
        _ if path.starts_with("/file/") => handle_file(request, root, &path["/file/".len()..]),
        _ => respond_text(request, 404, "not found", "text/plain; charset=utf-8"),
    }
}

/// `/file/{path}`：只读普通文件。`decoded_path` 已由上层做过一次 URL decode。
///
/// - `\` 归一成 `/`；
/// - 含 `..` 段一律 403；
/// - `C:/...`、`/C:/...`（以及 Unix 风格 `/...`）按绝对路径处理（任务要求兼容），
///   其余相对 `root`；
/// - 相对路径 canonicalize 后必须仍在 root 内（防符号链接逃逸）。
fn handle_file(request: Request, root: &Path, decoded_path: &str) -> Result<(), String> {
    let normalized = decoded_path.replace('\\', "/");

    if normalized.is_empty() || normalized.contains('\0') {
        return respond_text(request, 400, "bad path", "text/plain; charset=utf-8");
    }
    if normalized.split('/').any(|seg| seg == "..") {
        return respond_text(
            request,
            403,
            "forbidden: path traversal",
            "text/plain; charset=utf-8",
        );
    }

    let trimmed = normalized.trim_start_matches('/');
    let windows_abs = has_windows_drive(trimmed);
    let unix_abs = !windows_abs && Path::new(&normalized).is_absolute();
    let is_absolute = windows_abs || unix_abs;
    let candidate: PathBuf = if windows_abs {
        PathBuf::from(trimmed)
    } else if unix_abs {
        PathBuf::from(&normalized)
    } else {
        root.join(&normalized)
    };

    let canonical = match candidate.canonicalize() {
        Ok(path) => path,
        Err(_) => return respond_text(request, 404, "file not found", "text/plain; charset=utf-8"),
    };

    if !is_absolute {
        // root 与 candidate 均已 canonicalize；starts_with 按路径组件比较。
        if !canonical.starts_with(root) {
            return respond_text(
                request,
                403,
                "forbidden: outside clan root",
                "text/plain; charset=utf-8",
            );
        }
    }

    let meta = match std::fs::metadata(&canonical) {
        Ok(meta) => meta,
        Err(_) => return respond_text(request, 404, "file not found", "text/plain; charset=utf-8"),
    };
    if !meta.is_file() {
        return respond_text(
            request,
            404,
            "not a regular file",
            "text/plain; charset=utf-8",
        );
    }

    let total = meta.len();
    let media = is_media_path(&canonical);
    // 非媒体文件维持原来的 10MB 内存上限；媒体文件允许按 Range 流式读取，
    // 供本地播放器 seek（尤其是 moov 在文件尾部的 MP4）。
    if total as usize > MAX_RESPONSE_BYTES && !media {
        return respond_text(request, 413, "file too large", "text/plain; charset=utf-8");
    }

    let mime = mime_for(&canonical);
    let range_header = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Range"))
        .map(|header| header.value.as_str().to_string());

    if let Some(raw) = range_header {
        return match parse_byte_range(&raw, total) {
            Ok(range) => respond_file_range(request, &canonical, Some(range), total, mime),
            Err(()) => respond_range_not_satisfiable(request, total),
        };
    }

    if media && total as usize > MAX_RESPONSE_BYTES {
        // 大媒体文件：不整文件读进内存，直接流式返回
        return respond_file_range(request, &canonical, None, total, mime);
    }

    let data = match std::fs::read(&canonical) {
        Ok(data) => data,
        Err(e) => {
            return respond_text(
                request,
                500,
                format!("read file failed: {e}"),
                "text/plain; charset=utf-8",
            );
        }
    };
    if data.len() > MAX_RESPONSE_BYTES {
        return respond_text(request, 413, "file too large", "text/plain; charset=utf-8");
    }

    respond_with_mime(request, 200, data, mime)
}

/// 支持 Range 的媒体扩展名（小写比较）。字幕/音频也走流式，便于拖动。
fn is_media_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "mp4"
            | "m4v"
            | "mkv"
            | "webm"
            | "mov"
            | "avi"
            | "flv"
            | "wmv"
            | "ts"
            | "m2ts"
            | "mts"
            | "mpg"
            | "mpeg"
            | "m2v"
            | "m1v"
            | "rmvb"
            | "rm"
            | "3gp"
            | "3g2"
            | "ogv"
            | "asf"
            | "vob"
            | "f4v"
            | "tp"
            | "trp"
            | "m3u8"
            | "mp3"
            | "m4a"
            | "m4b"
            | "mka"
            | "aac"
            | "flac"
            | "wav"
            | "aiff"
            | "aif"
            | "ogg"
            | "opus"
            | "ac3"
            | "eac3"
            | "dts"
            | "ape"
            | "wv"
            | "tta"
            | "srt"
            | "ass"
            | "ssa"
            | "vtt"
    )
}

/// 一个闭区间字节范围（end 含）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

/// 解析单段 `Range: bytes=...`。返回 `Err(())` 表示范围不合法（应回 416）。
///
/// 只支持单段范围；多段（逗号）返回 416，避免实现 multipart/byteranges。
fn parse_byte_range(raw: &str, total: u64) -> Result<ByteRange, ()> {
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        return Err(());
    };
    if total == 0 || spec.contains(',') {
        return Err(());
    }
    let spec = spec.trim();
    let (start_raw, end_raw) = spec.split_once('-').ok_or(())?;
    let (start, end) = if start_raw.is_empty() {
        // bytes=-N：最后 N 字节
        let suffix: u64 = end_raw.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (total.saturating_sub(suffix), total - 1)
    } else {
        let start: u64 = start_raw.parse().map_err(|_| ())?;
        if start >= total {
            return Err(());
        }
        let end = if end_raw.is_empty() {
            total - 1
        } else {
            end_raw.parse::<u64>().map_err(|_| ())?.min(total - 1)
        };
        if end < start {
            return Err(());
        }
        (start, end)
    };
    Ok(ByteRange { start, end })
}

/// 返回 416，并带上 `Content-Range: bytes */total`。
fn respond_range_not_satisfiable(request: Request, total: u64) -> Result<(), String> {
    let content_type = Header::from_bytes(&b"Content-Type"[..], b"text/plain; charset=utf-8")
        .map_err(|_| "invalid content-type header".to_string())?;
    let content_range =
        Header::from_bytes(&b"Content-Range"[..], format!("bytes */{total}").as_bytes())
            .map_err(|_| "invalid content-range header".to_string())?;
    let response = Response::new(
        StatusCode(416),
        vec![content_type, content_range],
        std::io::Cursor::new(Vec::<u8>::new()),
        Some(0),
        None,
    );
    request.respond(response).map_err(|e| e.to_string())
}

/// 流式返回文件（可带单段 Range），不把整个文件读进内存。
fn respond_file_range(
    request: Request,
    path: &Path,
    range: Option<ByteRange>,
    total: u64,
    mime: &str,
) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| format!("open file failed: {e}"))?;
    let (status, start, end) = match range {
        Some(range) => (206, range.start, range.end),
        None => (200, 0, total.saturating_sub(1)),
    };
    let len = if total == 0 { 0 } else { end - start + 1 };

    let mut headers = vec![
        Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
            .map_err(|_| "invalid content-type header".to_string())?,
        Header::from_bytes(&b"Accept-Ranges"[..], b"bytes")
            .map_err(|_| "invalid accept-ranges header".to_string())?,
    ];
    if range.is_some() {
        headers.push(
            Header::from_bytes(
                &b"Content-Range"[..],
                format!("bytes {start}-{end}/{total}").as_bytes(),
            )
            .map_err(|_| "invalid content-range header".to_string())?,
        );
    }

    if len > 0 {
        file.seek(SeekFrom::Start(start))
            .map_err(|e| format!("seek file failed: {e}"))?;
    }
    let reader = file.take(len);
    let response = Response::new(
        StatusCode(status),
        headers,
        reader,
        Some(len as usize),
        None,
    )
    // 不要分块传输：mpv/ffmpeg 对 Content-Length + Range 的处理比 chunked 更稳，
    // 也便于测试直接按字节数断言。
    .with_chunked_threshold(usize::MAX);
    request.respond(response).map_err(|e| e.to_string())
}

/// 复用一个图片代理客户端：首页一次要拉几十张封面，复用连接池能省掉重复 TLS 握手。
fn image_client() -> Result<&'static reqwest::blocking::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::USER_AGENT,
                reqwest::header::HeaderValue::from_static(IMG_UA),
            );
            headers.insert(
                reqwest::header::REFERER,
                reqwest::header::HeaderValue::from_static(IMG_REFERER),
            );
            reqwest::blocking::Client::builder()
                .timeout(IMG_TIMEOUT)
                .default_headers(headers)
                .build()
                .map_err(|e| format!("create image client failed: {e}"))
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// `/img?url=<百分号编码的图片地址>`：豆瓣图片代理。
///
/// WebView 里直接加载 `img*.doubanio.com` 会被豆瓣按 418 拒绝（缺 Referer），
/// 所以统一走本地代理补上 UA/Referer。只允许豆瓣域名，避免变成任意 SSRF 跳板。
fn handle_img(request: Request, query: &str) -> Result<(), String> {
    let params = parse_query(query);
    let url = params.get("url").map(String::as_str).unwrap_or("").trim();
    if url.is_empty() {
        return respond_text(request, 400, "missing url", "text/plain; charset=utf-8");
    }
    if !is_allowed_image_url(url) {
        return respond_text(
            request,
            403,
            "forbidden: only douban.com / doubanio.com images are allowed",
            "text/plain; charset=utf-8",
        );
    }

    let client = match image_client() {
        Ok(client) => client,
        Err(e) => {
            return respond_text(
                request,
                502,
                format!("create image client failed: {e}"),
                "text/plain; charset=utf-8",
            );
        }
    };

    let response = match client.get(url).send() {
        Ok(response) => response,
        Err(e) => {
            return respond_text(
                request,
                502,
                format!("image fetch failed: {e}"),
                "text/plain; charset=utf-8",
            );
        }
    };

    if !response.status().is_success() {
        return respond_text(
            request,
            502,
            format!("image upstream status {}", response.status()),
            "text/plain; charset=utf-8",
        );
    }
    if let Some(length) = response.content_length() {
        if length as usize > MAX_RESPONSE_BYTES {
            return respond_text(request, 502, "image too large", "text/plain; charset=utf-8");
        }
    }

    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= 200 && !value.contains(['\r', '\n']))
        .unwrap_or_else(|| "image/jpeg".to_string());

    // 边读边限长：不能先 `bytes()` 把超大响应整个读进内存。
    let mut body = Vec::new();
    if let Err(e) = response
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut body)
    {
        return respond_text(
            request,
            502,
            format!("read image body failed: {e}"),
            "text/plain; charset=utf-8",
        );
    }
    if body.is_empty() {
        return respond_text(
            request,
            502,
            "image body is empty",
            "text/plain; charset=utf-8",
        );
    }
    if body.len() > MAX_RESPONSE_BYTES {
        return respond_text(request, 502, "image too large", "text/plain; charset=utf-8");
    }

    respond_image(request, body, &mime)
}

/// 图片代理域名白名单：douban.com / doubanio.com 及其子域。
fn is_allowed_image_url(url: &str) -> bool {
    let rest = match url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    {
        Some(rest) => rest,
        None => return false,
    };
    // authority 到第一个 / ? # 为止；去掉 userinfo（`douban.com@evil.com` 的 host 是 evil.com）。
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = host_port
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.');
    if host.is_empty() {
        return false;
    }
    let host = host.to_ascii_lowercase();
    host == "douban.com"
        || host.ends_with(".douban.com")
        || host == "doubanio.com"
        || host.ends_with(".doubanio.com")
}

fn respond_image(request: Request, body: Vec<u8>, mime: &str) -> Result<(), String> {
    let content_type = Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
        .map_err(|_| "invalid content-type header".to_string())?;
    let cache_control = Header::from_bytes(&b"Cache-Control"[..], IMG_CACHE_CONTROL.as_bytes())
        .map_err(|_| "invalid cache-control header".to_string())?;
    let response = Response::from_data(body)
        .with_status_code(StatusCode(200))
        .with_header(content_type)
        .with_header(cache_control);
    request
        .respond(response)
        .map_err(|e| format!("write response failed: {e}"))
}

/// `/proxy?do=live&type=txt&ext=<base64|url>`：直播订阅代理。
fn handle_proxy(request: Request, query: &str) -> Result<(), String> {
    let params = parse_query(query);
    let action = params.get("do").map(String::as_str).unwrap_or("");
    match action {
        "live" => handle_proxy_live(request, &params),
        "js" => handle_proxy_js(request, &params),
        other => respond_text(
            request,
            400,
            format!("unsupported do={other:?} (expect do=live|do=js)"),
            "text/plain; charset=utf-8",
        ),
    }
}

/// `/proxy?do=live&type=txt&ext=...`：直播订阅代理。
fn handle_proxy_live(request: Request, params: &HashMap<String, String>) -> Result<(), String> {
    let ext = params.get("ext").map(String::as_str).unwrap_or("").trim();
    if ext.is_empty() {
        return respond_text(request, 400, "missing ext", "text/plain; charset=utf-8");
    }

    let url = match decode_ext_url(ext) {
        Some(url) => url,
        None => {
            return respond_text(
                request,
                400,
                "invalid ext: expect http(s) URL or Base64(URL_SAFE|NO_WRAP) of it",
                "text/plain; charset=utf-8",
            );
        }
    };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return respond_text(
            request,
            400,
            "ext must be http(s)",
            "text/plain; charset=utf-8",
        );
    }

    let mut rules = vbox_core::model::Rules::default();
    rules.timeout = 15_000;
    rules.retry = 1;
    let client = match vbox_core::http::client_for(&rules) {
        Ok(client) => client,
        Err(e) => {
            return respond_text(
                request,
                502,
                format!("create http client failed: {e}"),
                "text/plain; charset=utf-8",
            );
        }
    };

    let empty_headers = HashMap::new();
    match vbox_core::http::get_text(&client, &url, &empty_headers, rules.retry) {
        Ok(text) => {
            if text.len() > MAX_RESPONSE_BYTES {
                return respond_text(
                    request,
                    502,
                    "upstream response too large",
                    "text/plain; charset=utf-8",
                );
            }
            respond_text(request, 200, text, "text/plain; charset=utf-8")
        }
        Err(e) => respond_text(
            request,
            502,
            format!("proxy fetch failed: {e}"),
            "text/plain; charset=utf-8",
        ),
    }
}

/// `/proxy?do=js&...`：JS spider 代理回环入口。
///
/// 调用顺序（对齐 TVBox `ApiConfig.proxyLocal` / `JsLoader.proxyInvoke`）：
/// 1. `siteKey` 已注册且脚本实现了 `proxy(params)` → 用脚本返回的响应；
/// 2. 否则按 `url` + `header` 直通上游（GET，透传 Range，上限 10MB）；
/// 3. 返回 m3u8 时把分片/密钥/变体地址重写成同一代理入口。
fn handle_proxy_js(request: Request, params: &HashMap<String, String>) -> Result<(), String> {
    let site_key = params
        .get("siteKey")
        .or_else(|| params.get("site_key"))
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    let mut call_params = serde_json::Map::new();
    for (key, value) in params {
        call_params.insert(key.clone(), serde_json::Value::String(value.clone()));
    }
    if !site_key.is_empty() {
        if let Some(reply) = vbox_core::proxy::call_js_proxy(site_key, &call_params) {
            return respond_proxy_reply(request, reply, params);
        }
        // 未注册或脚本没有实现 proxy：继续走直通
    }

    let url = params.get("url").map(String::as_str).unwrap_or("").trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return respond_text(
            request,
            400,
            "missing url (expect http(s)://...)",
            "text/plain; charset=utf-8",
        );
    }

    let mut rules = vbox_core::model::Rules::default();
    rules.timeout = 20_000;
    rules.retry = 1;
    let client = match vbox_core::http::client_for(&rules) {
        Ok(client) => client,
        Err(e) => {
            return respond_text(
                request,
                502,
                format!("create http client failed: {e}"),
                "text/plain; charset=utf-8",
            );
        }
    };
    let mut request_builder = client.get(url);
    let mut headers: HashMap<String, String> = HashMap::new();
    headers.insert(
        "User-Agent".to_string(),
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36".to_string(),
    );
    if let Some(raw) = params.get("header") {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) {
            for (key, value) in map {
                if let Some(value) = value.as_str() {
                    headers.insert(key, value.to_string());
                }
            }
        }
    }
    for (key, value) in &headers {
        request_builder = request_builder.header(key, value);
    }
    if let Some(range) = params.get("range").or_else(|| params.get("Range")) {
        request_builder = request_builder.header("Range", range);
    }
    let response = match request_builder.send() {
        Ok(response) => response,
        Err(e) => {
            return respond_text(
                request,
                502,
                format!("proxy fetch failed: {e}"),
                "text/plain; charset=utf-8",
            );
        }
    };
    let status = response.status().as_u16();
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = match response.bytes() {
        Ok(bytes) => bytes.to_vec(),
        Err(e) => {
            return respond_text(
                request,
                502,
                format!("read upstream body failed: {e}"),
                "text/plain; charset=utf-8",
            );
        }
    };
    if bytes.len() > MAX_RESPONSE_BYTES {
        return respond_text(
            request,
            502,
            "upstream response too large",
            "text/plain; charset=utf-8",
        );
    }
    if vbox_core::proxy::looks_like_m3u8(&mime, &bytes) {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            let base = vbox_core::proxy::local_base().unwrap_or_default();
            let query_without_url = query_without_url(params);
            let rewritten = vbox_core::proxy::rewrite_m3u8(url, text, &query_without_url, &base);
            return respond_with_mime(
                request,
                status,
                rewritten.into_bytes(),
                "application/vnd.apple.mpegurl",
            );
        }
    }
    respond_with_mime(request, status, bytes, &mime)
}

/// 响应脚本返回的代理结果（302 重定向 / 原始字节 / m3u8 重写）。
fn respond_proxy_reply(
    request: Request,
    reply: vbox_core::proxy::ProxyReply,
    params: &HashMap<String, String>,
) -> Result<(), String> {
    if let Some(location) = reply.location {
        return respond_redirect(request, 302, &location);
    }
    let mime = reply.mime;
    if vbox_core::proxy::looks_like_m3u8(&mime, &reply.body) {
        if let Ok(text) = std::str::from_utf8(&reply.body) {
            let base = vbox_core::proxy::local_base().unwrap_or_default();
            let query_without_url = query_without_url(params);
            let playlist_url = params.get("url").map(String::as_str).unwrap_or("");
            let rewritten = vbox_core::proxy::rewrite_m3u8(playlist_url, text, &query_without_url, &base);
            return respond_with_mime(
                request,
                reply.status,
                rewritten.into_bytes(),
                "application/vnd.apple.mpegurl",
            );
        }
    }
    respond_with_mime(request, reply.status, reply.body, &mime)
}

fn respond_redirect(request: Request, status: u16, location: &str) -> Result<(), String> {
    let header = Header::from_bytes(&b"Location"[..], location.as_bytes())
        .map_err(|_| "invalid location header".to_string())?;
    let response = Response::empty(StatusCode(status)).with_header(header);
    request
        .respond(response)
        .map_err(|e| format!("write redirect failed: {e}"))
}

/// 从已解码的 query 参数重建代理查询串（去掉 `url`，避免嵌套重复）。
fn query_without_url(params: &HashMap<String, String>) -> String {
    let mut pairs: Vec<String> = params
        .iter()
        .filter(|(key, _)| key.as_str() != "url")
        .map(|(key, value)| {
            format!(
                "{}={}",
                vbox_core::proxy::encode_param(key),
                vbox_core::proxy::encode_param(value)
            )
        })
        .collect();
    pairs.sort();
    pairs.join("&")
}

/* ---------------- 工具 ---------------- */

fn split_url(url: &str) -> (String, String) {
    let no_fragment = url.split('#').next().unwrap_or(url);
    match no_fragment.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (no_fragment.to_string(), String::new()),
    }
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let key = percent_decode_str(key).decode_utf8_lossy().into_owned();
        let value = percent_decode_str(value).decode_utf8_lossy().into_owned();
        map.insert(key, value);
    }
    map
}

/// `ext` 解码：已是 http(s) 直接用；否则按 Base64（URL_SAFE 优先，兼容标准 Base64）。
fn decode_ext_url(ext: &str) -> Option<String> {
    if ext.starts_with("http://") || ext.starts_with("https://") {
        return Some(ext.to_string());
    }

    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let mut decoded: Vec<Vec<u8>> = Vec::new();
    if let Ok(bytes) = URL_SAFE.decode(ext) {
        decoded.push(bytes);
    }
    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(ext) {
        decoded.push(bytes);
    }
    if let Ok(bytes) = STANDARD.decode(ext) {
        decoded.push(bytes);
    }
    if let Ok(bytes) = STANDARD_NO_PAD.decode(ext) {
        decoded.push(bytes);
    }

    decoded.into_iter().find_map(|bytes| {
        String::from_utf8(bytes).ok().and_then(|text| {
            let text = text.trim().to_string();
            if text.starts_with("http://") || text.starts_with("https://") {
                Some(text)
            } else {
                None
            }
        })
    })
}

fn has_windows_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn mime_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "json" => "application/json; charset=utf-8",
        "txt" | "log" => "text/plain; charset=utf-8",
        "m3u" | "m3u8" => "application/vnd.apple.mpegurl",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "flv" => "video/x-flv",
        "m2ts" | "mts" => "video/mp2t",
        "ts" => "video/mp2t",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "srt" => "application/x-subrip",
        "ass" | "ssa" => "text/x-ssa",
        "vtt" => "text/vtt",
        _ => "application/octet-stream",
    }
}

fn respond_text(
    request: Request,
    status: u16,
    body: impl Into<String>,
    mime: &str,
) -> Result<(), String> {
    respond_with_mime(request, status, body.into().into_bytes(), mime)
}

fn respond_with_mime(
    request: Request,
    status: u16,
    body: Vec<u8>,
    mime: &str,
) -> Result<(), String> {
    let header = Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
        .map_err(|_| "invalid content-type header".to_string())?;
    let response = Response::from_data(body)
        .with_status_code(StatusCode(status))
        .with_header(header);
    request
        .respond(response)
        .map_err(|e| format!("write response failed: {e}"))
}

/* ---------------- 单元测试 ---------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let seq = TEST_SEQ.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "vbox-local-server-{}-{}-{}",
                tag,
                std::process::id(),
                seq
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    /// 测试用 HLS 代理（缓存目录放在 test root 下，随 TestDir 一起清理）。
    fn hls_for(root: &Path) -> Arc<HlsProxy> {
        let prefs = Arc::new(crate::playback_cache::PlaybackCacheStore::new(root));
        Arc::new(HlsProxy::new(root.join("hls-cache"), prefs))
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 极简 HTTP/1.1 GET；返回 (status, 原始响应文本)。
    fn http_get(port: u16, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect local server");
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            path
        );
        stream.write_all(request.as_bytes()).expect("write request");
        let _ = stream.flush();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).expect("read response");
        let text = String::from_utf8_lossy(&buf).into_owned();
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        (status, text)
    }

    fn spawn_upstream(body: &'static str) -> (u16, JoinHandle<()>) {
        let server = Server::http(("127.0.0.1", 0)).expect("upstream bind");
        let port = server.server_addr().to_ip().expect("upstream addr").port();
        let handle = std::thread::spawn(move || {
            if let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(5)) {
                let _ = request.respond(Response::from_string(body));
            }
        });
        (port, handle)
    }

    #[test]
    fn health_ok() {
        let dir = TestDir::new("health");
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");
        let (status, body) = http_get(server.port, "/health");
        assert_eq!(status, 200);
        assert!(body.contains("ok"), "unexpected body: {body}");
        server.shutdown();
    }

    #[test]
    fn file_serves_and_blocks_traversal() {
        let dir = TestDir::new("file");
        std::fs::write(dir.path().join("hello world.json"), b"{\"hello\":1}").expect("write file");
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");

        let (status, body) = http_get(server.port, "/file/hello%20world.json");
        assert_eq!(status, 200, "body: {body}");
        assert!(body.contains("\"hello\":1"));
        assert!(body.to_ascii_lowercase().contains("application/json"));

        let (status, _) = http_get(server.port, "/file/../secret.txt");
        assert_eq!(status, 403, "path traversal must be forbidden");

        let (status, _) = http_get(server.port, "/file/not-exists.txt");
        assert_eq!(status, 404);

        server.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn file_serves_absolute_windows_path() {
        let dir = TestDir::new("abs");
        let file = dir.path().join("abs.txt");
        std::fs::write(&file, b"absolute-ok").expect("write file");
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");

        let absolute = file.to_string_lossy().replace('\\', "/");
        let (status, body) = http_get(server.port, &format!("/file/{absolute}"));
        assert_eq!(status, 200, "body: {body}");
        assert!(body.contains("absolute-ok"));

        server.shutdown();
    }

    #[test]
    fn byte_range_parser_handles_forms() {
        assert_eq!(
            parse_byte_range("bytes=0-9", 100),
            Ok(ByteRange { start: 0, end: 9 })
        );
        assert_eq!(
            parse_byte_range("bytes=90-", 100),
            Ok(ByteRange { start: 90, end: 99 })
        );
        assert_eq!(
            parse_byte_range("bytes=-5", 100),
            Ok(ByteRange { start: 95, end: 99 })
        );
        // 后缀比文件大：返回整个文件
        assert_eq!(
            parse_byte_range("bytes=-500", 100),
            Ok(ByteRange { start: 0, end: 99 })
        );
        // 尾部超出：截到文件末尾
        assert_eq!(
            parse_byte_range("bytes=99-999", 100),
            Ok(ByteRange { start: 99, end: 99 })
        );
        assert!(parse_byte_range("bytes=100-", 100).is_err());
        assert!(parse_byte_range("bytes=5-4", 100).is_err());
        assert!(parse_byte_range("bytes=-0", 100).is_err());
        assert!(parse_byte_range("bytes=0-1,3-4", 100).is_err());
        assert!(parse_byte_range("items=0-1", 100).is_err());
        assert!(parse_byte_range("bytes=0-1", 0).is_err());
    }

    #[test]
    fn file_supports_range_for_local_media() {
        let dir = TestDir::new("file-range");
        let bytes: Vec<u8> = (0..200u8).collect();
        std::fs::write(dir.path().join("media.mp4"), &bytes).expect("write media");
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");

        // 普通 GET：整文件
        let (status, head, body) = http_get_raw(server.port, "/file/media.mp4", None);
        assert_eq!(status, 200);
        assert_eq!(body, bytes);
        assert!(
            head.to_ascii_lowercase()
                .contains("content-type: video/mp4"),
            "head: {head}"
        );

        // 中段 Range：206 + Content-Range
        let (status, head, body) =
            http_get_raw(server.port, "/file/media.mp4", Some("bytes=10-19"));
        assert_eq!(status, 206);
        assert_eq!(body, bytes[10..=19].to_vec());
        let lower = head.to_ascii_lowercase();
        assert!(
            lower.contains("content-range: bytes 10-19/200"),
            "head: {head}"
        );
        assert!(lower.contains("accept-ranges: bytes"), "head: {head}");

        // 尾部 Range：seek 到 moov 所在位置
        let (status, head, body) = http_get_raw(server.port, "/file/media.mp4", Some("bytes=-8"));
        assert_eq!(status, 206);
        assert_eq!(body, bytes[192..].to_vec());
        assert!(
            head.to_ascii_lowercase()
                .contains("content-range: bytes 192-199/200"),
            "head: {head}"
        );

        // 越界 Range：416 + bytes */total
        let (status, head, body) = http_get_raw(server.port, "/file/media.mp4", Some("bytes=999-"));
        assert_eq!(status, 416);
        assert!(body.is_empty());
        assert!(
            head.to_ascii_lowercase()
                .contains("content-range: bytes */200"),
            "head: {head}"
        );

        server.shutdown();
    }

    #[test]
    fn file_streams_large_media() {
        let dir = TestDir::new("file-large");
        let path = dir.path().join("large.mp4");
        let size = MAX_RESPONSE_BYTES as u64 + 1024;
        let file = File::create(&path).expect("create large media");
        file.set_len(size).expect("sparse large media");
        drop(file);
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");

        assert!(
            size as usize > MAX_RESPONSE_BYTES,
            "前置条件：应超过内存上限"
        );

        // 大媒体不带 Range：流式 200，而不是 413
        let (status, head, body) = http_get_raw(server.port, "/file/large.mp4", None);
        assert_eq!(status, 200, "head: {head}");
        assert_eq!(body.len() as u64, size);

        // 只取头部：206，避免整文件传输
        let (status, _head, body) = http_get_raw(server.port, "/file/large.mp4", Some("bytes=0-3"));
        assert_eq!(status, 206);
        assert_eq!(body.len(), 4);

        server.shutdown();
    }

    #[test]
    fn proxy_decodes_base64_live_url() {
        let (upstream_port, upstream) = spawn_upstream("hello-live\n");
        let url = format!("http://127.0.0.1:{upstream_port}/live.txt");
        let ext = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(url.as_bytes());

        let dir = TestDir::new("proxy");
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");
        let (status, body) = http_get(server.port, &format!("/proxy?do=live&type=txt&ext={ext}"));
        assert_eq!(status, 200, "body: {body}");
        assert!(body.contains("hello-live"));

        server.shutdown();
        let _ = upstream.join();
    }

    #[test]
    fn proxy_rejects_unknown_do() {
        let dir = TestDir::new("proxy-do");
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");
        let (status, _) = http_get(server.port, "/proxy?do=site&ext=x");
        assert_eq!(status, 400);
        server.shutdown();
    }

    #[test]
    fn img_rejects_missing_and_foreign_urls() {
        let dir = TestDir::new("img");
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");

        let (status, _) = http_get(server.port, "/img");
        assert_eq!(status, 400, "missing url should be 400");

        let (status, _) = http_get(
            server.port,
            "/img?url=https%3A%2F%2Fevil.example.com%2Fa.jpg",
        );
        assert_eq!(status, 403, "non-douban host should be forbidden");

        // 借 userinfo 伪装成豆瓣域名：真实 host 仍是 evil.com。
        let (status, _) = http_get(
            server.port,
            "/img?url=https%3A%2F%2Fdouban.com%40evil.example.com%2Fa.jpg",
        );
        assert_eq!(status, 403, "userinfo trick should be forbidden");

        let (status, _) = http_get(server.port, "/img?url=file%3A%2F%2F%2Fetc%2Fpasswd");
        assert_eq!(status, 403, "non-http(s) should be forbidden");

        server.shutdown();
    }

    #[test]
    fn img_host_allowlist() {
        assert!(is_allowed_image_url(
            "https://img3.doubanio.com/view/photo/m_ratio_poster/public/p1.jpg"
        ));
        assert!(is_allowed_image_url("http://doubanio.com/a.jpg"));
        assert!(is_allowed_image_url("https://douban.com/a.jpg"));
        assert!(is_allowed_image_url("https://movie.douban.com/a.jpg"));

        assert!(!is_allowed_image_url("http://evil.com/douban.com/a.jpg"));
        assert!(!is_allowed_image_url("http://evil-douban.com/a.jpg"));
        assert!(!is_allowed_image_url("http://douban.com.evil.com/a.jpg"));
        assert!(!is_allowed_image_url("ftp://img3.doubanio.com/a.jpg"));
        assert!(!is_allowed_image_url("https:///a.jpg"));
    }

    /// 真实网络测试（默认不跑）：
    /// `cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored --nocapture img_proxies_real_douban_image`
    #[test]
    #[ignore = "真实网络测试：需要能访问 doubanio.com"]
    fn img_proxies_real_douban_image() {
        let target = "https://img3.doubanio.com/view/photo/m_ratio_poster/public/p2934286660.jpg";
        let encoded =
            percent_encoding::utf8_percent_encode(target, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        let dir = TestDir::new("img-real");
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");
        let (status, body) = http_get(server.port, &format!("/img?url={encoded}"));
        let head: String = body.chars().take(200).collect();
        assert_eq!(status, 200, "body head: {head}");
        let lower = body.to_ascii_lowercase();
        assert!(
            lower.contains("content-type: image/"),
            "缺少图片 Content-Type"
        );
        assert!(lower.contains("cache-control: public, max-age=86400"));
        assert!(body.len() > 10_000, "图片字节数异常: {}", body.len());
        println!("img proxy ok: {} bytes", body.len());
        server.shutdown();
    }

    /* ---------------- HLS 边下边播代理 ---------------- */

    /// 极简 HTTP GET（原始字节，可选 Range）：返回 (status, headers 文本, body)。
    fn http_get_raw(port: u16, path: &str, range: Option<&str>) -> (u16, String, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect local server");
        let mut request =
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        if let Some(range) = range {
            request.push_str(&format!("Range: {range}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).expect("write request");
        let _ = stream.flush();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).expect("read response");
        let split = buf
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("response headers end");
        let head = String::from_utf8_lossy(&buf[..split]).into_owned();
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        (status, head, buf[split + 4..].to_vec())
    }

    /// 测试用 AES-128 密钥/IV 与密文分片（与 mock 上游保持一致）
    const AES_KEY: [u8; 16] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        0x00,
    ];
    const AES_IV: [u8; 16] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ];

    fn encrypted_segment(plain: &[u8]) -> Vec<u8> {
        crate::hls_util::encrypt_for_test(plain, &AES_KEY, &AES_IV)
    }

    /// 模拟一个 HLS 源：master → 变体；加密变体（AES-128）+ 未加密变体各一套。
    fn spawn_hls_upstream() -> (u16, Arc<Mutex<HashMap<String, usize>>>) {
        let server = Server::http(("127.0.0.1", 0)).expect("hls upstream bind");
        let port = server
            .server_addr()
            .to_ip()
            .expect("hls upstream addr")
            .port();
        let counts: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let counter = Arc::clone(&counts);
        let enc1 = encrypted_segment(b"AAAASEGONE");
        let enc2 = encrypted_segment(b"BBBSEGTWO");
        std::thread::spawn(move || {
            while let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(15)) {
                let path = request.url().to_string();
                if let Ok(mut map) = counter.lock() {
                    *map.entry(path.clone()).or_insert(0) += 1;
                }
                let response = match path.as_str() {
                    "/master.m3u8" => Response::from_string(
                        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nv0.m3u8\n",
                    ),
                    // 加密变体：显式 IV，分片是密文
                    "/v0.m3u8" => Response::from_string(
                        "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x000102030405060708090a0b0c0d0e0f\n#EXTINF:2.0,\nenc1.ts\n#EXTINF:2.0,\nenc2.ts\n#EXT-X-ENDLIST\n",
                    ),
                    // 未加密的 HLS：用于「边看边存」对照
                    "/clear.m3u8" => Response::from_string(
                        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nclear-v0.m3u8\n",
                    ),
                    "/clear-v0.m3u8" => Response::from_string(
                        "#EXTM3U\n#EXTINF:2.0,\nseg1.ts\n#EXTINF:2.0,\nseg2.ts\n#EXT-X-ENDLIST\n",
                    ),
                    "/seg1.ts" => Response::from_data(b"AAAASEGONE".to_vec()),
                    "/seg2.ts" => Response::from_data(b"BBBSEGTWO".to_vec()),
                    "/enc1.ts" => Response::from_data(enc1.clone()),
                    "/enc2.ts" => Response::from_data(enc2.clone()),
                    "/key.bin" => Response::from_data(AES_KEY.to_vec()),
                    // BYTERANGE：两个分片指向同一个文件的不同区间
                    "/range.m3u8" => Response::from_string(
                        "#EXTM3U\n#EXT-X-VERSION:4\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nrange.bin\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10\nrange.bin\n#EXT-X-ENDLIST\n",
                    ),
                    "/range.bin" => Response::from_data(b"0123456789ABCDEFGHIJ".to_vec()),
                    _ => Response::from_string("not found").with_status_code(404),
                };
                let _ = request.respond(response);
            }
        });
        (port, counts)
    }

    fn hls_path(url: &str) -> String {
        let index = url.find("/hls/").expect("local hls url");
        url[index..].to_string()
    }

    fn first_hls_url(text: &str) -> String {
        text.lines()
            .find(|line| line.contains("/hls/"))
            .map(|line| line.trim().to_string())
            .expect("playlist 里没有本地 URL")
    }

    fn uri_attribute(line: &str) -> String {
        let start = line.find("URI=\"").expect("URI 属性") + 5;
        let end = line[start..].find('"').expect("URI 结束引号") + start;
        line[start..end].to_string()
    }

    /// HLS 代理端到端：master→variant 改写、分片按需下载并落盘复用、AES key 代理、Range。
    #[test]
    fn hls_proxy_rewrites_playlists_and_caches_segments() {
        let (upstream_port, counts) = spawn_hls_upstream();
        let dir = TestDir::new("hls");
        let hls = hls_for(dir.path());
        let server = start(dir.path().to_path_buf(), Arc::clone(&hls)).expect("start server");

        let source = format!("http://127.0.0.1:{upstream_port}/master.m3u8");
        let session = hls
            .create_session(&source, &[])
            .expect("create hls session");

        // master：应改写成本地变体地址
        let (status, _, body) = http_get_raw(server.port, &hls_path(&session.url), None);
        assert_eq!(status, 200);
        let master = String::from_utf8_lossy(&body).into_owned();
        let variant_url = first_hls_url(&master);
        assert!(variant_url.contains("/hls/"), "{master}");

        // variant：应改写成本地分片与本地 key
        let (status, _, body) = http_get_raw(server.port, &hls_path(&variant_url), None);
        assert_eq!(status, 200);
        let media = String::from_utf8_lossy(&body).into_owned();
        let seg_urls: Vec<String> = media
            .lines()
            .filter(|line| line.contains("/hls/") && line.contains("/s/"))
            .map(|line| line.trim().to_string())
            .collect();
        assert_eq!(seg_urls.len(), 2, "分片数量异常: {media}");
        let key_line = media
            .lines()
            .find(|line| line.starts_with("#EXT-X-KEY"))
            .expect("key 行");
        let key_url = uri_attribute(key_line);
        assert!(key_url.contains("/hls/"), "{key_line}");

        // 分片 1：首次走上游，二次命中本地缓存（代理层返回密文，解密在导出时做）
        let enc1 = encrypted_segment(b"AAAASEGONE");
        let (status, _, seg1) = http_get_raw(server.port, &hls_path(&seg_urls[0]), None);
        assert_eq!(status, 200);
        assert_eq!(seg1, enc1);
        let (status, _, seg1_again) = http_get_raw(server.port, &hls_path(&seg_urls[0]), None);
        assert_eq!(status, 200);
        assert_eq!(seg1_again, enc1);
        assert_eq!(
            counts.lock().unwrap().get("/enc1.ts").copied().unwrap_or(0),
            1,
            "第二次分片请求应命中缓存，不应再打上游"
        );

        // 分片 2 + AES key
        let (status, _, seg2) = http_get_raw(server.port, &hls_path(&seg_urls[1]), None);
        assert_eq!(status, 200);
        assert_eq!(seg2, encrypted_segment(b"BBBSEGTWO"));
        let (status, _, key) = http_get_raw(server.port, &hls_path(&key_url), None);
        assert_eq!(status, 200);
        assert_eq!(key, AES_KEY.to_vec());

        // AES-128：导出时应解密并合并为明文
        let save_dir = dir.path().join("saved");
        let saved = hls
            .export(&session.id, &save_dir, "加密剧集 第1集")
            .expect("AES-128 HLS 应支持解密保存");
        assert_eq!(
            saved.extension().and_then(|ext| ext.to_str()),
            Some("ts"),
            "{}",
            saved.display()
        );
        let decrypted = std::fs::read(&saved).expect("读取保存文件失败");
        assert_eq!(decrypted, b"AAAASEGONEBBBSEGTWO");
        assert_eq!(
            counts.lock().unwrap().get("/enc1.ts").copied().unwrap_or(0),
            1,
            "导出应复用已缓存分片，不重复下载"
        );

        // 未加密 HLS：边看边存（已缓存分片复用、未看分片补下后合并）
        let clear_source = format!("http://127.0.0.1:{upstream_port}/clear.m3u8");
        let clear = hls
            .create_session(&clear_source, &[])
            .expect("create clear session");
        let (_, _, body) = http_get_raw(server.port, &hls_path(&clear.url), None);
        let clear_variant = first_hls_url(&String::from_utf8_lossy(&body));
        let (_, _, body) = http_get_raw(server.port, &hls_path(&clear_variant), None);
        let clear_media = String::from_utf8_lossy(&body).into_owned();
        let clear_segments: Vec<String> = clear_media
            .lines()
            .filter(|line| line.contains("/s/"))
            .map(|line| line.trim().to_string())
            .collect();
        assert_eq!(clear_segments.len(), 2, "{clear_media}");
        // 只播第一段：导出时应复用 seg1、补下 seg2
        http_get_raw(server.port, &hls_path(&clear_segments[0]), None);
        let saved = hls
            .export(&clear.id, &save_dir, "测试剧集 第1集")
            .expect("保存 HLS 会话失败");
        assert_eq!(
            saved.extension().and_then(|ext| ext.to_str()),
            Some("ts"),
            "普通 TS HLS 应保存为 .ts: {}",
            saved.display()
        );
        let merged = std::fs::read(&saved).expect("读取保存文件失败");
        assert_eq!(merged, b"AAAASEGONEBBBSEGTWO");
        assert_eq!(
            counts.lock().unwrap().get("/seg1.ts").copied().unwrap_or(0),
            1,
            "导出应复用已缓存的明文分片（只下载过一次）"
        );
        assert_eq!(
            counts.lock().unwrap().get("/seg2.ts").copied().unwrap_or(0),
            1,
            "导出应补下未缓存分片（只下载一次）"
        );

        // Range：只取前 4 字节，返回 206 + Content-Range（代理层是密文）
        let (status, head, partial) =
            http_get_raw(server.port, &hls_path(&seg_urls[0]), Some("bytes=0-3"));
        assert_eq!(status, 206, "{head}");
        assert_eq!(partial, enc1[..4].to_vec());
        assert!(
            head.to_ascii_lowercase().contains("content-range"),
            "{head}"
        );

        hls.close_session(&session.id);
        server.shutdown();
    }

    /// BYTERANGE：两个分片指向同一文件的不同区间；播放按 Range 切片，导出按范围拼接且只下载一次。
    #[test]
    fn hls_proxy_exports_byterange_slices() {
        let (upstream_port, counts) = spawn_hls_upstream();
        let dir = TestDir::new("hls-range");
        let hls = hls_for(dir.path());
        let server = start(dir.path().to_path_buf(), Arc::clone(&hls)).expect("start server");

        let source = format!("http://127.0.0.1:{upstream_port}/range.m3u8");
        let session = hls
            .create_session(&source, &[])
            .expect("create range session");
        // 模拟 mpv 起播：拉一次播放列表，让代理记录分片顺序
        let (status, _, body) = http_get_raw(server.port, &hls_path(&session.url), None);
        assert_eq!(status, 200);
        let media = String::from_utf8_lossy(&body).into_owned();
        let seg_urls: Vec<String> = media
            .lines()
            .filter(|line| line.contains("/s/"))
            .map(|line| line.trim().to_string())
            .collect();
        assert_eq!(seg_urls.len(), 2, "{media}");

        // 播放：客户端只取第一个区间（Range），代理返回对应字节
        let (status, head, first) =
            http_get_raw(server.port, &hls_path(&seg_urls[0]), Some("bytes=0-9"));
        assert_eq!(status, 206, "{head}");
        assert_eq!(first, b"0123456789");

        // 导出：两个区间按顺序拼成完整文件，且同一资源只下载一次
        let saved = hls
            .export(&session.id, &dir.path().join("saved"), "区间测试")
            .expect("导出 BYTERANGE 失败");
        let merged = std::fs::read(&saved).expect("读取保存文件失败");
        assert_eq!(merged, b"0123456789ABCDEFGHIJ");
        assert_eq!(
            counts
                .lock()
                .unwrap()
                .get("/range.bin")
                .copied()
                .unwrap_or(0),
            1,
            "同一资源只应下载一次"
        );

        hls.close_session(&session.id);
        server.shutdown();
    }
    #[test]
    fn proxy_js_callback_route() {
        let dir = TestDir::new("proxy-js");
        vbox_core::proxy::register_js_proxy(
            "local-server-js-site",
            "function proxy(params) { return JSON.stringify({ code: 200, mime: 'text/plain', content: 'js:' + params.url }); }".to_string(),
            "local-server-js".to_string(),
            vbox_core::model::Rules::default(),
            HashMap::new(),
            None,
            None,
        );
        let server = start(dir.path().to_path_buf(), hls_for(dir.path())).expect("start server");
        let (status, body) = http_get(
            server.port,
            "/proxy?do=js&from=catvod&siteKey=local-server-js-site&url=https%3A%2F%2Fcdn.example.com%2Fa.m3u8",
        );
        vbox_core::proxy::clear_js_proxies();
        assert_eq!(status, 200, "body: {body}");
        assert!(body.contains("js:https://cdn.example.com/a.m3u8"), "body: {body}");
    }

}
