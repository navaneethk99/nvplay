//! Minimal bindings to the stable libmpv client and OpenGL render APIs.
//! Keep ABI declarations here; the UI never handles raw pointers.
use libloading::Library;
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    path::PathBuf,
    sync::Arc,
};

#[repr(C)]
pub struct Event {
    pub id: c_int,
    pub error: c_int,
    pub userdata: u64,
    pub data: *mut c_void,
}
#[repr(C)]
pub struct Property {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}
#[repr(C)]
pub struct EndFile {
    pub reason: c_int,
    pub error: c_int,
}
#[repr(C)]
pub struct LogMessage {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union NodeValue {
    pub string: *const c_char,
    pub flag: c_int,
    pub integer: i64,
    pub double: f64,
    pub list: *const NodeList,
}
#[repr(C)]
pub struct Node {
    pub value: NodeValue,
    pub format: c_int,
}
#[repr(C)]
pub struct NodeList {
    pub len: c_int,
    pub values: *const Node,
    pub keys: *const *const c_char,
}
#[repr(C)]
pub struct Param {
    pub kind: c_int,
    pub data: *mut c_void,
}
impl Param {
    pub fn new<T>(kind: c_int, value: &mut T) -> Self {
        Self {
            kind,
            data: (value as *mut T).cast(),
        }
    }
    pub fn end() -> Self {
        Self {
            kind: 0,
            data: std::ptr::null_mut(),
        }
    }
}
#[cfg(all(test, target_os = "macos"))]
#[repr(C)]
#[derive(Default)]
pub struct FrameInfo {
    pub flags: u64,
    pub target_time: i64,
}

#[repr(C)]
pub struct GlInit {
    pub get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub ctx: *mut c_void,
}
#[repr(C)]
pub struct Fbo {
    pub fbo: c_int,
    pub width: c_int,
    pub height: c_int,
    pub internal_format: c_int,
}

macro_rules! api {
    ($( $(#[$attr:meta])* $field:ident: $ty:ty),* $(,)?) => {
        pub struct Api { $($(#[$attr])* pub $field: $ty,)* _library: Library }
        impl Api {
            unsafe fn from_library(library: Library) -> Result<Self, String> {
                // SAFETY: signatures mirror mpv/client.h and mpv/render.h. The library
                // is retained until every client and renderer has been destroyed.
                unsafe { Ok(Self { $($(#[$attr])* $field: *library.get::<$ty>(concat!("mpv_", stringify!($field), "\0").as_bytes()).map_err(|e| e.to_string())?,)* _library: library }) }
            }
        }
    };
}
api! {
    create: unsafe extern "C" fn() -> *mut c_void,
    initialize: unsafe extern "C" fn(*mut c_void) -> c_int,
    terminate_destroy: unsafe extern "C" fn(*mut c_void),
    set_option_string: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    command_async: unsafe extern "C" fn(*mut c_void, u64, *const *const c_char) -> c_int,
    observe_property: unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int,
    #[cfg(test)]
    get_property_string: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    #[cfg(test)]
    free: unsafe extern "C" fn(*mut c_void),
    get_property_async: unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int,
    request_log_messages: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int,
    set_wakeup_callback: unsafe extern "C" fn(*mut c_void, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    wait_event: unsafe extern "C" fn(*mut c_void, f64) -> *const Event,
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    render_context_create: unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *mut Param) -> c_int,
    render_context_render: unsafe extern "C" fn(*mut c_void, *mut Param) -> c_int,
    render_context_set_update_callback: unsafe extern "C" fn(*mut c_void, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    #[cfg(all(test, target_os = "macos"))]
    render_context_get_info: unsafe extern "C" fn(*mut c_void, Param) -> c_int,
    render_context_update: unsafe extern "C" fn(*mut c_void) -> u64,
    render_context_free: unsafe extern "C" fn(*mut c_void),
}
impl Api {
    #[cfg(all(test, target_os = "macos"))]
    pub fn frame_time_us(&self, handle: *mut c_void, target: i64) -> i64 {
        type Clock = unsafe extern "C" fn(*mut c_void) -> i64;
        // mpv 0.41's renderer returns nanoseconds despite its stale header
        // comment. Older releases return microseconds and have no ns clock.
        unsafe {
            if let Ok(clock) = self._library.get::<Clock>(b"mpv_get_time_ns\0") {
                (clock(handle) - target) / 1000
            } else {
                let clock = self._library.get::<Clock>(b"mpv_get_time_us\0").unwrap();
                clock(handle) - target
            }
        }
    }
    pub fn load() -> Result<Arc<Self>, String> {
        let mut candidates = Vec::new();
        if let Some(path) = std::env::var_os("NVPLAY_MPV_LIBRARY") {
            candidates.push(PathBuf::from(path));
        }
        #[cfg(target_os = "macos")]
        let names = [
            "/opt/homebrew/lib/libmpv.dylib",
            "/usr/local/lib/libmpv.dylib",
            "libmpv.2.dylib",
            "libmpv.dylib",
        ];
        #[cfg(target_os = "linux")]
        let names = ["libmpv.so.2", "libmpv.so.1", "libmpv.so"];
        #[cfg(target_os = "windows")]
        let names = ["mpv-2.dll", "libmpv-2.dll", "mpv-1.dll"];
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let names: [&str; 0] = [];
        // Search the executable's directory for distributable app-local libraries.
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            candidates.extend(names.iter().map(|name| parent.join(name)));
        }
        candidates.extend(names.iter().map(PathBuf::from));
        let mut failures = Vec::new();
        for path in candidates {
            // SAFETY: loading native code is required for playback. Only documented
            // library locations or the user's explicit override are searched.
            let result = unsafe {
                Library::new(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|lib| Self::from_library(lib))
            };
            match result {
                Ok(api) => return Ok(Arc::new(api)),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        Err(format!(
            "libmpv could not be loaded. Install mpv or set NVPLAY_MPV_LIBRARY to its shared library.\n\n{}",
            failures.join("\n")
        ))
    }
    pub fn check(&self, code: c_int) -> Result<(), String> {
        if code >= 0 {
            Ok(())
        } else {
            // SAFETY: mpv_error_string returns a static, null-terminated string.
            Err(unsafe { CStr::from_ptr((self.error_string)(code)) }
                .to_string_lossy()
                .into_owned())
        }
    }
}
