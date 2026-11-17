//! SMB 分享的本机等价物。
//!
//! 上游 `SambaShare` 依赖 Android 侧的 `aliproxy` 伴生服务（`127.0.0.1:10078/smb/list|download`）。
//! 桌面端不引入 SMB2/NTLMv2 协议栈，改为复用本机已有的 SMB 挂载：
//! - Windows：`\\host\share\dir` UNC 路径（凭据由系统已建立的会话提供），或已映射的网络驱动器；
//! - macOS / Linux：已挂载的共享目录。
//!
//! 站点 ext 中通过 `smb_root` 指定映射根目录（`\\host\share` 或 `Z:` 或 `/mnt/share`），
//! 其中的第一层子目录名对应 SMB 的 share 名。

use std::path::{Path, PathBuf};

/// SMB 分享路径：`[user:pass@]host/share[/dir...]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbPath {
    pub user: String,
    pub pass: String,
    pub host: String,
    pub share: String,
    /// 相对 share 的子目录，`/` 分隔，无首尾斜杠。
    pub dir: String,
}

impl SmbPath {
    /// 解析分享路径。支持 `smb://` 前缀、`user:pass@` 凭据、Windows UNC（`\\host\share\dir`）。
    pub fn parse(input: &str) -> Option<Self> {
        let mut raw = input.trim();
        if raw.is_empty() {
            return None;
        }
        if let Some(rest) = raw.strip_prefix("smb://") {
            raw = rest;
        }
        // Windows UNC：\\host\share\dir
        if let Some(rest) = raw.strip_prefix(r"\\") {
            let mut parts = rest.split(['\\', '/']).filter(|s| !s.is_empty());
            let host = parts.next()?.to_string();
            let share = parts.next()?.to_string();
            let dir = parts.collect::<Vec<_>>().join("/");
            return Some(Self { user: String::new(), pass: String::new(), host, share, dir });
        }
        let (creds, rest) = match raw.rfind('@') {
            Some(idx) => (&raw[..idx], &raw[idx + 1..]),
            None => ("", raw),
        };
        let (user, pass) = match creds.find(':') {
            Some(idx) => (creds[..idx].to_string(), creds[idx + 1..].to_string()),
            None => (creds.to_string(), String::new()),
        };
        let mut parts = rest.split('/').filter(|s| !s.is_empty());
        let host = parts.next()?.to_string();
        let share = parts.next()?.to_string();
        let dir = parts.collect::<Vec<_>>().join("/");
        if host.is_empty() || share.is_empty() {
            return None;
        }
        Some(Self { user, pass, host, share, dir })
    }

    /// 规范化的分享地址（不含凭据），用于条目 id。
    pub fn share_url(&self) -> String {
        if self.dir.is_empty() {
            format!("{}/{}", self.host, self.share)
        } else {
            format!("{}/{}/{}", self.host, self.share, self.dir)
        }
    }

    /// 目录下的子路径。
    pub fn child(&self, rel: &str) -> Self {
        let rel = rel.trim_matches('/');
        let dir = if self.dir.is_empty() {
            rel.to_string()
        } else if rel.is_empty() {
            self.dir.clone()
        } else {
            format!("{}/{}", self.dir, rel)
        };
        Self { dir, ..self.clone() }
    }

    /// 上级目录；share 根返回 `None`。
    pub fn parent(&self) -> Option<Self> {
        if self.dir.is_empty() {
            return None;
        }
        let dir = match self.dir.rsplit_once('/') {
            Some((head, _)) => head.to_string(),
            None => String::new(),
        };
        Some(Self { dir, ..self.clone() })
    }

    /// 文件名（末段）。
    pub fn name(&self) -> &str {
        self.dir.rsplit('/').next().unwrap_or(&self.dir)
    }
}

/// 目录条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// 本机挂载根。
#[derive(Debug, Clone, Default)]
pub struct SmbBackend {
    root: Option<PathBuf>,
}

impl SmbBackend {
    pub fn new(root: Option<PathBuf>) -> Self {
        Self { root }
    }

    /// 从站点 ext 解析：JSON 的 `smb_root`，或纯文本整串。
    pub fn from_ext(ext: &str) -> Self {
        let text = ext.trim();
        if text.is_empty() {
            return Self::default();
        }
        if text.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                if let Some(root) = v.get("smb_root").and_then(|x| x.as_str()) {
                    return Self::new(Some(PathBuf::from(root.trim())));
                }
            }
        }
        Self::default()
    }

    pub fn has_root(&self) -> bool {
        self.root.is_some()
    }

    /// 解析为本地文件系统路径。
    pub fn local_path(&self, path: &SmbPath) -> PathBuf {
        if let Some(root) = &self.root {
            let mut p = root.join(&path.share);
            if !path.dir.is_empty() {
                for seg in path.dir.split('/') {
                    p.push(seg);
                }
            }
            return p;
        }
        #[cfg(windows)]
        {
            let mut p = PathBuf::from(format!(r"\\{}\{}", path.host, path.share));
            if !path.dir.is_empty() {
                for seg in path.dir.split('/') {
                    p.push(seg);
                }
            }
            return p;
        }
        #[cfg(not(windows))]
        {
            let mut p = PathBuf::from(format!("/mnt/{}/{}", path.host, path.share));
            if !path.dir.is_empty() {
                for seg in path.dir.split('/') {
                    p.push(seg);
                }
            }
            p
        }
    }

    /// 列目录（按名称排序，目录在前）。
    pub fn list(&self, path: &SmbPath) -> Result<Vec<SmbEntry>, String> {
        let dir = self.local_path(path);
        let rd = std::fs::read_dir(&dir)
            .map_err(|e| format!("SMB 目录不可读：{}（{}）", dir.display(), e))?;
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            out.push(SmbEntry { name, is_dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() } });
        }
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }

    /// 文件是否可读（播放前校验）。
    pub fn is_file(&self, path: &SmbPath) -> bool {
        self.local_path(path).is_file()
    }
}

/// 本地文件转为可播放 URL（`file:///` + 逐段百分号编码）。
pub fn file_url(path: &Path) -> String {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = String::from("file:///");
    let mut first = true;
    for comp in abs.components() {
        if let std::path::Component::Normal(seg) = comp {
            if !first {
                out.push('/');
            }
            first = false;
            out.push_str(&percent_encode(&seg.to_string_lossy()));
        }
    }
    out
}

fn percent_encode(seg: &str) -> String {
    let mut out = String::new();
    for b in seg.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vbox_smb_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_smb_paths() {
        let p = SmbPath::parse("smb://u:p@host/share/a/b.mp4").unwrap();
        assert_eq!(p.user, "u");
        assert_eq!(p.pass, "p");
        assert_eq!(p.host, "host");
        assert_eq!(p.share, "share");
        assert_eq!(p.dir, "a/b.mp4");
        assert_eq!(p.share_url(), "host/share/a/b.mp4");
        assert_eq!(p.parent().unwrap().share_url(), "host/share/a");

        let q = SmbPath::parse(r"\\nas\media\电影\a.mkv").unwrap();
        assert_eq!((q.host.as_str(), q.share.as_str(), q.dir.as_str()), ("nas", "media", "电影/a.mkv"));

        let r = SmbPath::parse("host/share").unwrap();
        assert_eq!(r.share_url(), "host/share");
        assert!(r.parent().is_none());
        assert_eq!(r.child("x/y").share_url(), "host/share/x/y");
        assert!(SmbPath::parse("").is_none());
        assert!(SmbPath::parse("onlyhost").is_none());
    }

    #[test]
    fn backend_from_ext_reads_json_root() {
        let b = SmbBackend::from_ext(r#"{"smb_root":"Z:/media"}"#);
        assert!(b.has_root());
        assert_eq!(b.local_path(&SmbPath::parse("h/s/d").unwrap()), PathBuf::from("Z:/media").join("s").join("d"));
        assert!(!SmbBackend::from_ext("").has_root());
        assert!(!SmbBackend::from_ext("plain text").has_root());
    }

    #[test]
    fn lists_directory_from_mapped_root() {
        let root = temp_dir("list");
        std::fs::create_dir_all(root.join("share/sub")).unwrap();
        std::fs::write(root.join("share/b.txt"), b"hello").unwrap();
        std::fs::write(root.join("share/a.mp4"), vec![0u8; 32]).unwrap();
        let backend = SmbBackend::new(Some(root.clone()));
        let entries = backend.list(&SmbPath::parse("host/share").unwrap()).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "a.mp4", "b.txt"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[2].size, 5);
        assert!(backend.list(&SmbPath::parse("host/missing").unwrap()).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn file_url_encodes_segments() {
        let root = temp_dir("url");
        let file = root.join("带 空格.mp4");
        std::fs::write(&file, b"x").unwrap();
        let url = file_url(&file);
        assert!(url.starts_with("file:///"));
        assert!(url.contains("%E5%B8%A6"));
        assert!(url.contains("%20"));
        std::fs::remove_dir_all(&root).ok();
    }
}
