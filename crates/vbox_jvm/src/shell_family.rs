//! 加固壳「家族指纹」（P4，数据驱动）。
//!
//! 目标：拿到一个可疑 jar 时，先判断它属于哪个**壳家族**，而不是逐个 jar 逆向。
//! 指纹只依赖两样东西：
//! 1. zip 条目名（`assets/*.so` 这类 native 解密库的名字最有辨识度）；
//! 2. dex 字节里的约定类/约定字符串（`DexNative`、`merge/...` 混淆装载器、`Shinidie`、
//!    类名后缀 `Guard`/`Amns`、配置加密类 `MyCrypto`）。
//!
//! 命中已知家族 → 复用现成的 shim 约定；命中未知家族 → 明确报「未适配」，
//! 而不是静默失败后让人再写一遍业务代码（那是旧 REA 老路）。

use std::path::Path;

use serde::Serialize;

use crate::dex;
use crate::errors::Result;

/// 一个 jar 的壳指纹。
#[derive(Debug, Clone, Serialize)]
pub struct ShellFingerprint {
    /// 是否判定为加固壳。
    pub shell: bool,
    /// 壳家族（`plain` 表示普通 jar）。
    pub family: String,
    /// 命中的约定标记。
    pub markers: Vec<String>,
    /// 包内 native 库（`assets/*.so` 等）。
    pub native_libs: Vec<String>,
    /// 观察到的类名后缀约定。
    pub suffixes: Vec<String>,
    /// 壳是否把业务配置解密也放在 native（纯 Java shim 解不开）。
    pub needs_native_crypto: bool,
    /// 给运维看的说明。
    pub notes: Vec<String>,
}

impl ShellFingerprint {
    /// 一句话摘要，CLI/日志直接用。
    pub fn summary(&self) -> String {
        if !self.shell {
            return "普通 jar（无加固壳特征）".to_string();
        }
        let libs = if self.native_libs.is_empty() {
            "无 native 库".to_string()
        } else {
            self.native_libs.join(",")
        };
        let sfx = if self.suffixes.is_empty() {
            "-".to_string()
        } else {
            self.suffixes.join("/")
        };
        format!(
            "加固壳 family={} native=[{}] 后缀=[{}] 配置加密={}",
            self.family,
            libs,
            sfx,
            if self.needs_native_crypto { "是" } else { "否" }
        )
    }
}

/// 单个 jar 的扫描上限（dex 解压后字节数），防止超大包拖死诊断命令。
const MAX_SCAN_BYTES: usize = 96 * 1024 * 1024;

/// 对本地 jar 做壳指纹。读不出来（坏 zip 等）时返回 `Err`，交给调用方显示。
pub fn fingerprint(path: &Path) -> Result<ShellFingerprint> {
    let bytes = std::fs::read(path)?;
    let entries = dex::entries(path)?;

    let mut native_libs = Vec::new();
    for e in &entries {
        let lower = e.name.to_ascii_lowercase();
        if lower.ends_with(".so") {
            native_libs.push(e.name.clone());
        }
    }
    native_libs.sort();
    native_libs.dedup();

    // 解出 dex 条目内容做字符串指纹（stored 直接切片、deflate 用 flate2）。
    let mut scanned = 0usize;
    let mut hay: Vec<u8> = Vec::new();
    for e in &entries {
        let lower = e.name.to_ascii_lowercase();
        if !(lower.ends_with(".dex") || lower.ends_with(".bin")) {
            continue;
        }
        if scanned >= MAX_SCAN_BYTES {
            break;
        }
        let end = (e.data_off + e.comp_size).min(bytes.len());
        if e.data_off >= end {
            continue;
        }
        let raw = &bytes[e.data_off..end];
        let part = if e.method == 0 { raw.to_vec() } else { inflate(raw).unwrap_or_default() };
        scanned += part.len();
        hay.extend_from_slice(&part);
    }

    let mut markers: Vec<String> = Vec::new();
    let mut has = |m: &str| -> bool {
        let hit = hay.windows(m.len()).any(|w| w == m.as_bytes());
        if hit {
            markers.push(m.to_string());
        }
        hit
    };
    let dexnative = has("DexNative");
    let obscured = has("merge/l1lIl1l1IlIlIl1l1I");
    let shinidie = has("Shinidie");
    let mycrypto = has("MyCrypto");
    let amns = has("Amns");
    let guard = has("Guard");

    let mut suffixes = Vec::new();
    if guard {
        suffixes.push("Guard".to_string());
    }
    if amns {
        suffixes.push("Amns".to_string());
    }
    if shinidie {
        suffixes.push("Shinidie".to_string());
    }

    // 家族判定：优先看 native 库名，其次看约定装载器。
    let libs_lower = native_libs.join(" ").to_ascii_lowercase();
    let family = if dexnative && !native_libs.is_empty() {
        if libs_lower.contains("guard") || libs_lower.contains("fty") {
            "ftyguard".to_string()
        } else if libs_lower.contains("enc") || libs_lower.contains("aw") {
            "aowu-enc".to_string()
        } else {
            "dexnative-native".to_string()
        }
    } else if dexnative {
        "dexnative".to_string()
    } else if obscured || shinidie {
        "obscured-loader".to_string()
    } else if amns || guard {
        "suffix-wrapper".to_string()
    } else {
        "plain".to_string()
    };

    let shell = family != "plain";
    let mut notes = Vec::new();
    if shell {
        notes.push("可用 shell-shim 纯 Java 快路径（约定类 com.github.catvod.spider.DexNative）".to_string());
        if mycrypto {
            notes.push("检测到 MyCrypto：若其方法是 native，则业务配置无法在纯 Java 下解密".to_string());
        }
        if native_libs.is_empty() {
            notes.push("未见 assets/*.so；native 解密库可能从别处加载或已内联".to_string());
        }
    }

    Ok(ShellFingerprint {
        shell,
        family,
        markers,
        native_libs,
        suffixes,
        needs_native_crypto: mycrypto,
        notes,
    })
}

/// zlib raw deflate 解压（zip 的 method=8 就是裸 deflate）。
fn inflate(raw: &[u8]) -> Option<Vec<u8>> {
    use flate2::read::DeflateDecoder;
    use std::io::Read;
    let mut out = Vec::new();
    DeflateDecoder::new(raw).read_to_end(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for b in data {
            c ^= *b as u32;
            for _ in 0..8 {
                let m = (c & 1) != 0;
                c >>= 1;
                if m {
                    c ^= 0xEDB8_8320;
                }
            }
        }
        !c
    }

    /// 造一个最小 zip（全 stored）：classes.dex + 一个 native 库名。
    fn make_zip(path: &Path, dex_bytes: &[u8], so_name: &str) {
        let mut out: Vec<u8> = Vec::new();
        let mut central: Vec<u8> = Vec::new();
        let mut add = |name: &str, data: &[u8]| {
            let off = out.len() as u32;
            let name_b = name.as_bytes();
            let crc = crc32(data);
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&[0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name_b);
            out.extend_from_slice(data);

            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&[20, 0]);
            central.extend_from_slice(&[20, 0, 0, 0]);
            central.extend_from_slice(&[0, 0]);
            central.extend_from_slice(&[0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&off.to_le_bytes());
            central.extend_from_slice(name_b);
        };
        add("classes.dex", dex_bytes);
        add(so_name, b"not-a-real-so");
        let cd_off = out.len() as u32;
        out.extend_from_slice(&central);
        let cd_size = central.len() as u32;
        let n = 2u16;
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn detects_plain_and_shellized_jars() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/vbox_jvm_family");
        std::fs::create_dir_all(&base).unwrap();

        let plain = base.join("plain.jar");
        make_zip(&plain, b"dex\n035\0com/github/catvod/spider/Foo", "assets/whatever.txt");
        let fp = fingerprint(&plain).unwrap();
        assert_eq!(fp.family, "plain");
        assert!(!fp.shell);

        let shell = base.join("shell.jar");
        make_zip(
            &shell,
            b"dex\n035\0com/github/catvod/spider/DexNative com/github/catvod/spider/MyCrypto",
            "assets/awenc-v7.so",
        );
        let fp = fingerprint(&shell).unwrap();
        assert!(fp.shell);
        assert_eq!(fp.family, "aowu-enc");
        assert!(fp.needs_native_crypto);
        assert!(fp.native_libs.iter().any(|n| n.contains("awenc")));
    }
}
