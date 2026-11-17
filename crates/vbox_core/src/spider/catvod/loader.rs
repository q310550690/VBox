//! jar 加载层（计划 §5.4「jar 生命周期」的桌面端实现）。
//!
//! 负责把配置里的 `jar` 字段变成「本地可读的 jar 文件」：
//! - 来源：`http(s)://` / `file://` / `clan://`（局域网易位 http）；`assets://` 明确报错（`JarLayout`）
//! - `;md5;<hex>`：下载后校验，失败回退旧缓存或报 `JarIntegrity`
//! - `img+`：伪装 jar，自动裁掉非 zip 前缀
//! - 无 md5：3 天 TTL，过期重下；下载失败回退旧缓存
//! - 落盘：`<root>/<md5 或 url-hash>.jar`，先写临时文件再原子改名
//!
//! 本模块不解析 dex、不执行 jar，只保证「有正确字节」。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::error::{CoreError, Result};

use super::contract::{CatvodError, ErrorClass, JarSpec};


/// 构造带 jar 字段原文的加载错误（错误文案里 `jar=` 必须能指回配置）。
fn jar_error(
    class: ErrorClass,
    site: &crate::model::Site,
    spec: &JarSpec,
    detail: impl Into<String>,
) -> CatvodError {
    CatvodError::new(class, site, detail).with_jar(spec.raw.clone())
}

/// ZIP 本地文件头魔数（jar 一定是 zip）。
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
/// 默认 TTL：无 md5 的 jar 视为 3 天有效（对接 catvod 行为）。
pub const DEFAULT_TTL: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// 单个 jar 允许下载的默认上限（超过则报错，而不是截断后缓存）。
pub const DEFAULT_MAX_JAR_BYTES: u64 = 64 * 1024 * 1024;
/// `img+` 伪装时，在文件头最多探测这么多字节找 zip 魔数。
const DISGUISE_SCAN_LIMIT: usize = 4096;
/// 下载临时文件序号：保证同一 jar 的并发下载不共用同一个 `.part` 文件。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 加载结果。
#[derive(Debug, Clone)]
pub struct LoadedJar {
    pub spec: JarSpec,
    pub path: PathBuf,
    pub bytes: u64,
    pub md5: String,
    pub from_cache: bool,
    /// `img+` 伪装被裁掉的前缀字节数（0 表示未裁剪）
    pub stripped_prefix: usize,
}

impl LoadedJar {
    pub fn is_zip(&self) -> bool {
        true // 加载阶段已校验
    }
}

/// jar 本地缓存。
#[derive(Debug, Clone)]
pub struct JarStore {
    root: PathBuf,
    ttl: Duration,
    max_bytes: u64,
}

impl JarStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ttl: DEFAULT_TTL,
            max_bytes: DEFAULT_MAX_JAR_BYTES,
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 解析字段（不访问网络）。
    pub fn spec(&self, raw: &str) -> Result<Option<JarSpec>> {
        JarSpec::parse(raw)
    }

    /// 加载 jar。`client` 由调用方传入（沿用 VBox 的 UA/超时/代理配置）。
    pub fn load(&self, raw: &str, site: &crate::model::Site, client: &reqwest::blocking::Client) -> Result<LoadedJar> {
        let spec = JarSpec::parse(raw)?.ok_or_else(|| {
            CatvodError::new(ErrorClass::JarLayout, site, "jar 字段为空").into_core()
        })?;
        self.load_spec(&spec, site, client)
    }

    pub fn load_spec(
        &self,
        spec: &JarSpec,
        site: &crate::model::Site,
        client: &reqwest::blocking::Client,
    ) -> Result<LoadedJar> {
        if spec.is_assets() {
            return Err(jar_error(ErrorClass::JarLayout, site, spec, format!("assets:// 在桌面端无对应资源包: {}", spec.url),
            )
            .into_core());
        }
        if let Some(path) = spec.local_path() {
            return self.load_local(spec, site, Path::new(&path));
        }
        if let Some(rest) = spec.url.strip_prefix("clan://") {
            // clan:// 是局域网文件服务约定；这里退化为 http://<rest>
            let url = format!("http://{rest}");
            let http_spec = JarSpec {
                raw: spec.raw.clone(),
                url,
                md5: spec.md5.clone(),
                disguised: spec.disguised,
            };
            return self.load_remote(&http_spec, site, client);
        }
        if spec.is_remote() {
            return self.load_remote(spec, site, client);
        }
        Err(jar_error(ErrorClass::JarLayout, site, spec, format!("无法识别的 jar 地址: {}", spec.url),
        )
        .into_core())
    }

    fn cache_path(&self, spec: &JarSpec) -> PathBuf {
        self.root.join(spec.cache_key())
    }

    fn load_local(
        &self,
        spec: &JarSpec,
        site: &crate::model::Site,
        path: &Path,
    ) -> Result<LoadedJar> {
        // 本地 jar 与远程共用同一个上限：仅供参考的 file:// 也不能无界读入内存。
        let meta = std::fs::metadata(path).map_err(|e| {
            jar_error(ErrorClass::JarFetch, site, spec, format!("读取本地 jar 失败 {}: {e}", path.display()),
            )
            .into_core()
        })?;
        if meta.len() > self.max_bytes {
            return Err(jar_error(ErrorClass::JarLayout, site, spec, format!("jar 超过上限 {} 字节: {}", self.max_bytes, path.display()),
            )
            .into_core());
        }
        let bytes = std::fs::read(path).map_err(|e| {
            jar_error(ErrorClass::JarFetch, site, spec, format!("读取本地 jar 失败 {}: {e}", path.display()),
            )
            .into_core()
        })?;
        if bytes.len() as u64 > self.max_bytes {
            return Err(jar_error(ErrorClass::JarLayout, site, spec, format!("jar 超过上限 {} 字节: {}", self.max_bytes, path.display()),
            )
            .into_core());
        }
        let (bytes, stripped) = normalize_zip_bytes(bytes, spec.disguised).map_err(|detail| {
            jar_error(ErrorClass::JarLayout, site, spec, detail).into_core()
        })?;
        let digest = md5::compute(&bytes);
        let actual = format!("{digest:x}");
        if let Some(expected) = &spec.md5 {
            if &actual != expected {
                return Err(jar_error(ErrorClass::JarIntegrity, site, spec, format!("md5 不符: 期望 {expected}, 实际 {actual}"),
                )
                .into_core());
            }
        }
        Ok(LoadedJar {
            spec: spec.clone(),
            path: path.to_path_buf(),
            bytes: bytes.len() as u64,
            md5: actual,
            from_cache: false,
            stripped_prefix: stripped,
        })
    }

    fn load_remote(
        &self,
        spec: &JarSpec,
        site: &crate::model::Site,
        client: &reqwest::blocking::Client,
    ) -> Result<LoadedJar> {
        let cache = self.cache_path(spec);
        let cached = read_cached(&cache, spec, self.ttl, self.max_bytes);

        match cached {
            Some(jar) => {
                tracing::debug!("catvod jar 命中缓存: {}", cache.display());
                return Ok(jar);
            }
            None => {}
        }

        match self.download(spec, site, client) {
            Ok(jar) => Ok(jar),
            Err(err) => {
                // 下载失败回退旧缓存（catvod 行为）
                if let Some(bytes) = read_bounded(&cache, self.max_bytes) {
                    if let Ok((bytes, stripped)) = normalize_zip_bytes(bytes, spec.disguised) {
                        let digest = md5::compute(&bytes);
                        let actual = format!("{digest:x}");
                        if spec.md5.as_deref().map_or(true, |m| m == actual) {
                            tracing::warn!(
                                "catvod jar 下载失败，回退旧缓存 {}: {err}",
                                cache.display()
                            );
                            return Ok(LoadedJar {
                                spec: spec.clone(),
                                path: cache,
                                bytes: bytes.len() as u64,
                                md5: actual,
                                from_cache: true,
                                stripped_prefix: stripped,
                            });
                        }
                    }
                }
                Err(err)
            }
        }
    }

    fn download(
        &self,
        spec: &JarSpec,
        site: &crate::model::Site,
        client: &reqwest::blocking::Client,
    ) -> Result<LoadedJar> {
        std::fs::create_dir_all(&self.root).map_err(|e| {
            jar_error(ErrorClass::JarFetch, site, spec, format!("创建 jar 缓存目录失败 {}: {e}", self.root.display()),
            )
            .into_core()
        })?;
        let resp = client.get(&spec.url).send().map_err(|e| {
            jar_error(ErrorClass::JarFetch, site, spec, format!("下载 jar 失败 {}: {e}", spec.url),
            )
            .into_core()
        })?;
        if !resp.status().is_success() {
            return Err(jar_error(ErrorClass::JarFetch, site, spec, format!("下载 jar HTTP {}: {}", resp.status(), spec.url),
            )
            .into_core());
        }
        // 多读 1 字节用于识别超限：截断后缓存会让后续 dex 解析莫名失败，
        // 而无 md5 的 jar 无法靠摘要发现截断。
        let mut buf = Vec::with_capacity(1 << 20);
        resp.take(self.max_bytes + 1)
            .read_to_end(&mut buf)
            .map_err(CoreError::Io)?;
        if buf.len() as u64 > self.max_bytes {
            return Err(jar_error(ErrorClass::JarLayout, site, spec, format!("jar 超过上限 {} 字节: {}", self.max_bytes, spec.url),
            )
            .into_core());
        }
        let (bytes, stripped) = normalize_zip_bytes(buf, spec.disguised).map_err(|detail| {
            jar_error(ErrorClass::JarLayout, site, spec, detail).into_core()
        })?;
        let digest = md5::compute(&bytes);
        let actual = format!("{digest:x}");
        if let Some(expected) = &spec.md5 {
            if &actual != expected {
                return Err(jar_error(ErrorClass::JarIntegrity, site, spec, format!("md5 不符: 期望 {expected}, 实际 {actual}（{}）", spec.url),
                )
                .into_core());
            }
        }
        let path = self.cache_path(spec);
        // 每个下载使用唯一临时名：同一 jar 被多个站点并发加载时，
        // 共用 `<md5>.jar.part` 会让两次 write 交错，rename 出损坏字节。
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("jar.{}.{}.part", std::process::id(), seq));
        if let Err(err) = std::fs::write(&tmp, &bytes) {
            // 部分写入（磁盘满/权限）会留下残缺 .part；必须清理，避免长期堆积。
            let _ = std::fs::remove_file(&tmp);
            return Err(jar_error(ErrorClass::JarFetch, site, spec, format!("写入 jar 缓存失败 {}: {err}", tmp.display()),
            )
            .into_core());
        }
        if let Err(err) = std::fs::rename(&tmp, &path) {
            // 并发下另一个线程可能已经写好有效缓存；只要校验通过就复用它。
            let _ = std::fs::remove_file(&tmp);
            if let Some(cached) = read_cached(&path, spec, self.ttl, self.max_bytes) {
                return Ok(cached);
            }
            return Err(jar_error(ErrorClass::JarFetch, site, spec, format!("重命名 jar 缓存失败 {}: {err}", path.display()),
            )
            .into_core());
        }
        tracing::info!(
            "catvod jar 已缓存 {} ({} bytes, md5={})",
            path.display(),
            bytes.len(),
            actual
        );
        Ok(LoadedJar {
            spec: spec.clone(),
            path,
            bytes: bytes.len() as u64,
            md5: actual,
            from_cache: false,
            stripped_prefix: stripped,
        })
    }
}

fn read_cached(cache: &Path, spec: &JarSpec, ttl: Duration, max_bytes: u64) -> Option<LoadedJar> {
    let meta = std::fs::metadata(cache).ok()?;
    if !meta.is_file() {
        return None;
    }
    let bytes = read_bounded(cache, max_bytes)?;
    let (bytes, stripped) = normalize_zip_bytes(bytes, spec.disguised).ok()?;
    let digest = md5::compute(&bytes);
    let actual = format!("{digest:x}");
    if let Some(expected) = &spec.md5 {
        if &actual != expected {
            tracing::warn!("cache md5 不符，忽略: {} ({actual})", cache.display());
            return None;
        }
    } else {
        let modified = meta.modified().ok()?;
        // 时钟回拨/备份还原/手工改 mtime 都可能让 mtime 落在未来。
        // 旧代码把 duration_since 的 Err 当成「缓存无效」，导致无 md5 的 jar
        // 每次加载都重下，且没有任何日志线索。这里按「刚写入」处理（age=0）。
        let age = match SystemTime::now().duration_since(modified) {
            Ok(age) => age,
            Err(_) => {
                tracing::warn!(
                    "catvod jar 缓存 mtime 在未来，按刚写入处理: {}",
                    cache.display()
                );
                Duration::ZERO
            }
        };
        if age > ttl {
            return None;
        }
    }
    Some(LoadedJar {
        spec: spec.clone(),
        path: cache.to_path_buf(),
        bytes: bytes.len() as u64,
        md5: actual,
        from_cache: true,
        stripped_prefix: stripped,
    })
}

/// 有界读取缓存文件：超过上限视为无效缓存（下载路径会覆盖它），
/// 避免损坏/被替换的超大缓存绕过与本地/远程一致的 max_bytes 约束。
fn read_bounded(cache: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(cache).ok()?;
    if !meta.is_file() || meta.len() > max_bytes {
        tracing::warn!(
            "缓存超过上限，忽略: {} ({} > {})",
            cache.display(),
            meta.len(),
            max_bytes
        );
        return None;
    }
    let bytes = std::fs::read(cache).ok()?;
    if bytes.len() as u64 > max_bytes {
        return None;
    }
    Some(bytes)
}

/// 校验并（必要时）裁剪伪装前缀，返回 (字节, 裁剪长度)。
fn normalize_zip_bytes(mut bytes: Vec<u8>, disguised: bool) -> std::result::Result<(Vec<u8>, usize), String> {
    if bytes.len() >= 4 && &bytes[..4] == ZIP_MAGIC {
        return Ok((bytes, 0));
    }
    if !disguised {
        return Err("jar 不是 zip（缺少 PK\\x03\\x04 头）".to_string());
    }
    let limit = bytes.len().min(DISGUISE_SCAN_LIMIT);
    if let Some(pos) = bytes[..limit]
        .windows(4)
        .position(|w| w == ZIP_MAGIC)
    {
        let stripped = bytes.split_off(pos);
        bytes = stripped;
        return Ok((bytes, pos));
    }
    Err("img+ 伪装 jar 前 4KB 内未找到 zip 头".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Site;

    fn site() -> Site {
        Site {
            key: "t".into(),
            name: "t".into(),
            kind: 3,
            api: "csp_T".into(),
            ..Default::default()
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vbox-catvod-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_jar() -> Vec<u8> {
        let mut v = ZIP_MAGIC.to_vec();
        v.extend_from_slice(b"catvod-test-payload");
        v
    }

    #[test]
    fn loads_local_jar_and_verifies_md5() {
        let dir = tmp_dir("local");
        let jar_path = dir.join("a.jar");
        let bytes = fake_jar();
        std::fs::write(&jar_path, &bytes).unwrap();
        let md5_hex = format!("{:x}", md5::compute(&bytes));
        let raw = format!("file://{};md5;{}", jar_path.display(), md5_hex);
        let store = JarStore::new(dir.join("cache"));
        let client = reqwest::blocking::Client::new();
        let loaded = store.load(&raw, &site(), &client).unwrap();
        assert_eq!(loaded.bytes, bytes.len() as u64);
        assert_eq!(loaded.md5, md5_hex);
        assert!(!loaded.from_cache);
    }

    #[test]
    fn rejects_local_jar_with_wrong_md5() {
        let dir = tmp_dir("md5");
        let jar_path = dir.join("a.jar");
        std::fs::write(&jar_path, fake_jar()).unwrap();
        let raw = format!(
            "file://{};md5;00000000000000000000000000000000",
            jar_path.display()
        );
        let store = JarStore::new(dir.join("cache"));
        let client = reqwest::blocking::Client::new();
        let err = store.load(&raw, &site(), &client).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("catvod.jar.integrity"), "{msg}");
    }

    #[test]
    fn strips_img_padding_when_disguised() {
        let dir = tmp_dir("img");
        let jar_path = dir.join("a.jpg");
        let mut disguised = b"\xFF\xD8\xFF\xE0".to_vec(); // JPEG 头
        disguised.extend_from_slice(&fake_jar());
        std::fs::write(&jar_path, &disguised).unwrap();
        let raw = format!("img+file://{}", jar_path.display());
        let store = JarStore::new(dir.join("cache"));
        let client = reqwest::blocking::Client::new();
        let loaded = store.load(&raw, &site(), &client).unwrap();
        assert_eq!(loaded.stripped_prefix, 4);
        assert_eq!(loaded.bytes, fake_jar().len() as u64);
    }

    #[test]
    fn assets_jar_is_classified_unsupported() {
        let dir = tmp_dir("assets");
        let store = JarStore::new(dir.join("cache"));
        let client = reqwest::blocking::Client::new();
        let err = store
            .load("assets://spider.jar", &site(), &client)
            .unwrap_err();
        assert!(err.to_string().contains("catvod.jar.layout"), "{err}");
    }
}
