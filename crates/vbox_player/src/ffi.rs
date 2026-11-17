//! libmpv C API 子集：运行时动态加载 `libmpv-2.dll`
//!
//! 只声明 VBox 播放器真正用到的接口，避免引入 `libmpv-sys` 这种需要链接期
//! import library 的依赖 —— DLL 找不到时返回明确错误，编译 / 启动都不受影响。
//!
//! 设计要点：
//! - `MpvApi` 持有 DLL 模块句柄与函数指针，`Arc` 共享，最后一个引用释放时 `FreeLibrary`；
//! - `mpv_wait_event` 只能在**一个**线程调用（见 `mpv.rs` 的事件线程），其它命令
//!   （loadfile / pause / seek ...）按 libmpv 文档可从任意线程调用；
//! - 字符串统一 UTF-8 传入（mpv 内部按 UTF-8 处理），Windows 路径由 `Player` 决定。

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::path::PathBuf;

use crate::mpv::PlayerError;

/* ---------------- 常量（mpv/client.h） ---------------- */

pub const MPV_FORMAT_NONE: c_int = 0;
pub const MPV_FORMAT_STRING: c_int = 1;
pub const MPV_FORMAT_FLAG: c_int = 3;
pub const MPV_FORMAT_INT64: c_int = 4;
pub const MPV_FORMAT_DOUBLE: c_int = 5;

pub const MPV_EVENT_NONE: c_int = 0;
pub const MPV_EVENT_SHUTDOWN: c_int = 1;
pub const MPV_EVENT_START_FILE: c_int = 6;
pub const MPV_EVENT_END_FILE: c_int = 7;
pub const MPV_EVENT_FILE_LOADED: c_int = 8;
pub const MPV_EVENT_VIDEO_RECONFIG: c_int = 17;
pub const MPV_EVENT_AUDIO_RECONFIG: c_int = 18;
pub const MPV_EVENT_SEEK: c_int = 20;
pub const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;

/// `mpv_end_file_reason`
pub const MPV_END_FILE_REASON_EOF: c_int = 0;
pub const MPV_END_FILE_REASON_ERROR: c_int = 4;

/// `mpv_event`（只读取 event_id / error / data；data 生命周期到下一次 wait_event）
#[repr(C)]
pub struct MpvEvent {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

/// `mpv_event_end_file`（END_FILE 事件的 data）
#[repr(C)]
pub struct MpvEventEndFile {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

/* ---------------- 函数指针表 ---------------- */

type CreateFn = unsafe extern "C" fn() -> *mut c_void;
type InitializeFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type TerminateDestroyFn = unsafe extern "C" fn(*mut c_void);
type SetOptionStringFn = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type SetPropertyStringFn =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type GetPropertyStringFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char;
type GetPropertyFn =
    unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int;
type CommandFn = unsafe extern "C" fn(*mut c_void, *const *const c_char) -> c_int;
type WaitEventFn = unsafe extern "C" fn(*mut c_void, c_double) -> *mut MpvEvent;
type ErrorStringFn = unsafe extern "C" fn(c_int) -> *const c_char;
type FreeFn = unsafe extern "C" fn(*mut c_void);

/// 动态加载后的 libmpv API 表。
pub struct MpvApi {
    module: *mut c_void,
    pub create: CreateFn,
    pub initialize: InitializeFn,
    pub terminate_destroy: TerminateDestroyFn,
    pub set_option_string: SetOptionStringFn,
    pub set_property_string: SetPropertyStringFn,
    pub get_property_string: GetPropertyStringFn,
    pub get_property: GetPropertyFn,
    command_fn: CommandFn,
    pub wait_event: WaitEventFn,
    pub error_string: ErrorStringFn,
    pub free: FreeFn,
}

unsafe impl Send for MpvApi {}
unsafe impl Sync for MpvApi {}

impl Drop for MpvApi {
    fn drop(&mut self) {
        if !self.module.is_null() {
            unsafe { free_library(self.module) };
            self.module = std::ptr::null_mut();
        }
    }
}

impl MpvApi {
    /// 按候选路径依次尝试加载（第一个成功的）。
    pub fn load_default() -> Result<Self, PlayerError> {
        let candidates = default_lib_paths();
        let mut tried = Vec::new();
        for path in &candidates {
            if !path.is_file() {
                continue;
            }
            match Self::load_from(path) {
                Ok(api) => return Ok(api),
                Err(err) => tried.push(format!("{}: {err}", path.display())),
            }
        }
        let mut message = format!("找不到可用的 {LIB_DISPLAY_NAME}");
        if !tried.is_empty() {
            message.push_str("（已尝试：");
            message.push_str(&tried.join("；"));
            message.push('）');
        } else {
            message.push_str("（已搜索：");
            message.push_str(
                &candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("；"),
            );
            message.push('）');
        }
        Err(PlayerError::Load(message))
    }

    /// 从指定路径加载。
    pub fn load_from(path: &std::path::Path) -> Result<Self, PlayerError> {
        let module = unsafe { load_library(path) }
            .map_err(|e| PlayerError::Load(format!("加载 {} 失败: {e}", path.display())))?;
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                let ptr = unsafe { get_proc_address(module, $name) };
                if ptr.is_null() {
                    unsafe { free_library(module) };
                    return Err(PlayerError::Load(format!("libmpv 缺少符号 {}", $name)));
                }
                unsafe { std::mem::transmute::<*mut c_void, $ty>(ptr) }
            }};
        }
        Ok(Self {
            module,
            create: sym!("mpv_create", CreateFn),
            initialize: sym!("mpv_initialize", InitializeFn),
            terminate_destroy: sym!("mpv_terminate_destroy", TerminateDestroyFn),
            set_option_string: sym!("mpv_set_option_string", SetOptionStringFn),
            set_property_string: sym!("mpv_set_property_string", SetPropertyStringFn),
            get_property_string: sym!("mpv_get_property_string", GetPropertyStringFn),
            get_property: sym!("mpv_get_property", GetPropertyFn),
            command_fn: sym!("mpv_command", CommandFn),
            wait_event: sym!("mpv_wait_event", WaitEventFn),
            error_string: sym!("mpv_error_string", ErrorStringFn),
            free: sym!("mpv_free", FreeFn),
        })
    }

    /* ---------------- 便捷封装 ---------------- */

    pub fn set_option(&self, handle: *mut c_void, name: &str, value: &str) -> Result<(), PlayerError> {
        let name = cstr(name)?;
        let value = cstr(value)?;
        let rc = unsafe { (self.set_option_string)(handle, name.as_ptr(), value.as_ptr()) };
        self.check(rc, "mpv_set_option_string")
    }

    pub fn set_property(&self, handle: *mut c_void, name: &str, value: &str) -> Result<(), PlayerError> {
        let name = cstr(name)?;
        let value = cstr(value)?;
        let rc = unsafe { (self.set_property_string)(handle, name.as_ptr(), value.as_ptr()) };
        self.check(rc, "mpv_set_property_string")
    }

    pub fn command(&self, handle: *mut c_void, args: &[&str]) -> Result<(), PlayerError> {
        let owned = args
            .iter()
            .map(|a| cstr(a))
            .collect::<Result<Vec<_>, _>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let rc = unsafe { (self.command_fn)(handle, ptrs.as_ptr()) };
        self.check(rc, "mpv_command")
    }

    pub fn get_double(&self, handle: *mut c_void, name: &str) -> Option<f64> {
        let name = CString::new(name).ok()?;
        let mut value: c_double = 0.0;
        let rc = unsafe {
            (self.get_property)(
                handle,
                name.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut value as *mut _ as *mut c_void,
            )
        };
        (rc >= 0).then_some(value)
    }

    pub fn get_flag(&self, handle: *mut c_void, name: &str) -> Option<bool> {
        let name = CString::new(name).ok()?;
        let mut value: c_int = 0;
        let rc = unsafe {
            (self.get_property)(
                handle,
                name.as_ptr(),
                MPV_FORMAT_FLAG,
                &mut value as *mut _ as *mut c_void,
            )
        };
        (rc >= 0).then_some(value != 0)
    }

    pub fn get_i64(&self, handle: *mut c_void, name: &str) -> Option<i64> {
        let name = CString::new(name).ok()?;
        let mut value: i64 = 0;
        let rc = unsafe {
            (self.get_property)(
                handle,
                name.as_ptr(),
                MPV_FORMAT_INT64,
                &mut value as *mut _ as *mut c_void,
            )
        };
        (rc >= 0).then_some(value)
    }

    pub fn get_string(&self, handle: *mut c_void, name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        let ptr = unsafe { (self.get_property_string)(handle, name.as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        unsafe { (self.free)(ptr as *mut c_void) };
        Some(text)
    }

    pub fn error_text(&self, code: c_int) -> String {
        let ptr = unsafe { (self.error_string)(code) };
        if ptr.is_null() {
            return format!("错误码 {code}");
        }
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }

    fn check(&self, rc: c_int, what: &str) -> Result<(), PlayerError> {
        if rc >= 0 {
            Ok(())
        } else {
            Err(PlayerError::Api(format!("{what} 失败: {}", self.error_text(rc))))
        }
    }
}

fn cstr(value: &str) -> Result<CString, PlayerError> {
    CString::new(value).map_err(|_| PlayerError::Api(format!("字符串包含 NUL 字节: {value}")))
}

/// 平台库文件名（Windows DLL / macOS dylib / Linux so）
#[cfg(windows)]
const LIB_NAMES: &[&str] = &["libmpv-2.dll"];
#[cfg(target_os = "macos")]
const LIB_NAMES: &[&str] = &["libmpv.2.dylib", "libmpv.dylib"];
#[cfg(all(unix, not(target_os = "macos")))]
const LIB_NAMES: &[&str] = &["libmpv.so.2", "libmpv.so"];
#[cfg(not(any(windows, unix)))]
const LIB_NAMES: &[&str] = &[];

#[cfg(windows)]
const LIB_DISPLAY_NAME: &str = "libmpv-2.dll";
#[cfg(target_os = "macos")]
const LIB_DISPLAY_NAME: &str = "libmpv.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const LIB_DISPLAY_NAME: &str = "libmpv.so";
#[cfg(not(any(windows, unix)))]
const LIB_DISPLAY_NAME: &str = "libmpv";

/// 候选 libmpv 路径（按优先级）：
/// 1. 环境变量 `VBOX_LIBMPV`（调试 / 自定义安装位置）
/// 2. exe 同目录（打包后由 Tauri resources 放在 exe 旁 / resources 目录）
/// 3. `resources/<lib>`（Tauri resource_dir 布局）
/// 4. macOS：Homebrew / usr-local 的 `libmpv*.dylib`
/// 5. 开发期：`src-tauri/resources/<lib>`（cargo run 时 exe 在 target/debug）
pub fn default_lib_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(custom) = std::env::var_os("VBOX_LIBMPV") {
        paths.push(PathBuf::from(custom));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in LIB_NAMES {
                paths.push(dir.join(name));
                paths.push(dir.join("resources").join(name));
                if let Some(near) = dir.parent().and_then(|p| p.parent()) {
                    paths.push(near.join("resources").join(name));
                    paths.push(near.join(name));
                }
            }
            // macOS .app 布局：Contents/MacOS/vbox-desktop → Contents/Resources/resources/...
            #[cfg(target_os = "macos")]
            {
                if let Some(contents) = dir.parent() {
                    for name in LIB_NAMES {
                        paths.push(contents.join("Resources").join("resources").join(name));
                        paths.push(contents.join("Resources").join(name));
                    }
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        for prefix in ["/opt/homebrew/lib", "/usr/local/lib"] {
            for name in LIB_NAMES {
                paths.push(PathBuf::from(prefix).join(name));
            }
        }
    }
    // 开发期直接指向仓库内资源目录（cargo run 时最稳）
    for name in LIB_NAMES {
        paths.push(
            PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../src-tauri/resources/"
            ))
            .join(name),
        );
    }
    paths
}

/* ---------------- Windows 动态加载 ---------------- */

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::path::Path;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(
            file_name: *const u16,
            file: *mut c_void,
            flags: u32,
        ) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    const LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: u32 = 0x0000_0100;
    const LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: u32 = 0x0000_1000;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    pub unsafe fn load_library(path: &Path) -> Result<*mut c_void, String> {
        let wide = wide(path);
        let module = LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        );
        if module.is_null() {
            return Err(format!("Win32 错误 {}", GetLastError()));
        }
        Ok(module)
    }

    pub unsafe fn get_proc_address(module: *mut c_void, name: &str) -> *mut c_void {
        let mut bytes = name.as_bytes().to_vec();
        bytes.push(0);
        GetProcAddress(module, bytes.as_ptr())
    }

    pub unsafe fn free_library(module: *mut c_void) {
        FreeLibrary(module);
    }
}

#[cfg(unix)]
mod platform {
    use std::ffi::{c_void, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub unsafe fn load_library(path: &Path) -> Result<*mut c_void, String> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "库路径包含 NUL 字节".to_string())?;
        let module = libc::dlopen(c_path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
        if module.is_null() {
            let err = libc::dlerror();
            let message = if err.is_null() {
                "未知 dlopen 错误".to_string()
            } else {
                std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned()
            };
            return Err(message);
        }
        Ok(module)
    }

    pub unsafe fn get_proc_address(module: *mut c_void, name: &str) -> *mut c_void {
        let Ok(c_name) = CString::new(name) else {
            return std::ptr::null_mut();
        };
        libc::dlsym(module, c_name.as_ptr())
    }

    pub unsafe fn free_library(module: *mut c_void) {
        libc::dlclose(module);
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use std::ffi::c_void;
    use std::path::Path;

    pub unsafe fn load_library(_path: &Path) -> Result<*mut c_void, String> {
        Err("当前平台暂不支持 libmpv 嵌入播放（仅 Windows / macOS / Linux）".to_string())
    }

    pub unsafe fn get_proc_address(_module: *mut c_void, _name: &str) -> *mut c_void {
        std::ptr::null_mut()
    }

    pub unsafe fn free_library(_module: *mut c_void) {}
}

use platform::{free_library, get_proc_address, load_library};
