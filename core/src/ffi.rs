//! C ABI. Every function returns a heap-allocated JSON string, `{"ok": ...}` or
//! `{"error": {"code": "...", "message": "..."}}`, which the caller frees with `andysu_free`.
//! Panics never cross the boundary. The same JSON API backs the JNI glue.

// Safety for every `unsafe extern "C"` function below: pointers must be NULL or valid, NUL-terminated
// UTF-8 strings (byte buffers: valid for the given length), as documented in andysu.h.
#![allow(clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::session::{Config, Session};
use crate::store::Store;
use crate::{Error, Result};

/// JSON-level API shared by the C ABI and JNI.
pub mod api {
    use super::*;

    static SESSIONS: Mutex<Option<HashMap<u64, Session>>> = Mutex::new(None);
    static NEXT: AtomicU64 = AtomicU64::new(1);

    pub fn wrap(r: Result<Value>) -> String {
        match r {
            Ok(v) => json!({ "ok": v }).to_string(),
            Err(e) => json!({ "error": { "code": e.code(), "message": e.to_string() } }).to_string(),
        }
    }

    pub fn guarded(f: impl FnOnce() -> Result<Value>) -> String {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(r) => wrap(r),
            Err(p) => {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                wrap(Err(Error::Usage(format!("internal panic: {msg}"))))
            }
        }
    }

    fn with<T>(h: u64, f: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
        let mut g = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
        let s =
            g.get_or_insert_with(HashMap::new).get_mut(&h).ok_or_else(|| Error::Usage(format!("no session {h}")))?;
        f(s)
    }

    fn r16(b: &[u8]) -> Result<[u8; 16]> {
        b.try_into().map_err(|_| Error::Usage("random16 must be 16 bytes".into()))
    }

    pub fn session_new(root: &str, config_json: &str, random16: &[u8]) -> Result<Value> {
        let cfg: Config = serde_json::from_str(config_json).map_err(|e| Error::Usage(format!("config: {e}")))?;
        let s = Session::new(Store::open(root)?, cfg, r16(random16)?)?;
        let h = NEXT.fetch_add(1, Ordering::Relaxed);
        SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(h, s);
        Ok(json!(h))
    }

    pub fn session_free(h: u64) {
        if let Some(m) = SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            m.remove(&h);
        }
    }

    pub fn check(h: u64, url: &str, m: &[u8], s: &[u8], etag: Option<String>, now: i64) -> Result<Value> {
        with(h, |x| x.check(url, m, s, etag, now)).map(|o| serde_json::to_value(o).unwrap())
    }

    pub fn check_cached(h: u64, now: i64) -> Result<Value> {
        with(h, |x| x.check_cached(now)).map(|o| serde_json::to_value(o).unwrap())
    }

    pub fn plan(h: u64) -> Result<Value> {
        with(h, |x| x.plan()).map(|o| serde_json::to_value(o).unwrap())
    }

    pub fn complete(h: u64, key: &str) -> Result<Value> {
        with(h, |x| x.complete(key)).map(|_| Value::Null)
    }

    pub fn skip_delta(h: u64, key: &str) -> Result<Value> {
        with(h, |x| {
            x.skip_delta(key);
            Ok(Value::Null)
        })
    }

    pub fn stage(h: u64) -> Result<Value> {
        with(h, |x| x.stage()).map(|o| serde_json::to_value(o).unwrap())
    }

    pub fn etag(root: &str) -> Result<Value> {
        let st = crate::state::load(&Store::open(root)?.state_path())?;
        Ok(json!(st.and_then(|s| s.last_manifest).and_then(|l| l.etag)))
    }

    pub fn launch(root: &str, random16: &[u8], now: i64, apply_pending: bool, max_attempts: u32) -> Result<Value> {
        let store = Store::open(root)?;
        let d = store.launch(r16(random16)?, now, apply_pending, max_attempts)?;
        let dir = d.slot.as_ref().map(|s| store.slot_dir(&s.id).to_string_lossy().to_string());
        let mut v = serde_json::to_value(d).unwrap();
        v["slot_dir"] = json!(dir);
        Ok(v)
    }

    pub fn mark_healthy(root: &str) -> Result<Value> {
        Ok(serde_json::to_value(Store::open(root)?.update_state(|s| s.mark_healthy())?).unwrap())
    }

    pub fn report_failure(root: &str, reason: &str) -> Result<Value> {
        let store = Store::open(root)?;
        let e = store.update_state(|s| s.report_failure(reason))?;
        let cur = crate::state::load(&store.state_path())?.and_then(|s| s.current);
        let dir = cur.as_ref().map(|s| store.slot_dir(&s.id).to_string_lossy().to_string());
        Ok(json!({ "event": e, "slot": cur, "slot_dir": dir }))
    }

    pub fn begin_live(root: &str, now: i64) -> Result<Value> {
        let store = Store::open(root)?;
        let e = store.update_state(|s| s.begin_live(now))??;
        let cur = crate::state::load(&store.state_path())?.and_then(|s| s.current);
        let dir = cur.as_ref().map(|s| store.slot_dir(&s.id).to_string_lossy().to_string());
        Ok(json!({ "event": e, "slot": cur, "slot_dir": dir }))
    }

    pub fn reset(root: &str, reason: &str) -> Result<Value> {
        Ok(serde_json::to_value(Store::open(root)?.update_state(|s| s.reset_to_factory(reason))?).unwrap())
    }

    pub fn gc(root: &str, low_storage: bool, keep_parts: &[String]) -> Result<Value> {
        let store = Store::open(root)?;
        let st = crate::state::load(&store.state_path())?.ok_or_else(|| Error::State("no state".into()))?;
        Ok(serde_json::to_value(store.gc(&st, low_storage, keep_parts)?).unwrap())
    }

    pub fn state(root: &str) -> Result<Value> {
        let store = Store::open(root)?;
        Ok(serde_json::to_value(crate::state::load(&store.state_path())?).unwrap())
    }

    pub fn crash_marker_path(root: &str) -> Result<Value> {
        Ok(json!(Store::open(root)?.crash_marker_path().to_string_lossy()))
    }

    /// What a host must load for a slot on `abi`: absolute library paths in load order, the
    /// entry library and symbol, and the dex class path. Works for packed slots (descriptor
    /// `native` map) and for unpacked factory payloads (falls back to lib/<abi>/<entry>.so).
    pub fn load_plan(slot_dir: &str, abi: &str) -> Result<Value> {
        let dir = std::path::Path::new(slot_dir);
        let dp = dir.join(crate::manifest::DESCRIPTOR_PATH);
        let d = crate::payload::Descriptor::parse(
            &std::fs::read(&dp).map_err(|e| Error::Io(format!("{}: {e}", dp.display())))?,
        )?;
        let abs = |p: &str| dir.join(p).to_string_lossy().to_string();
        let mut libs: Vec<String> =
            d.native.get(abi).map(|v| v.iter().map(|l| abs(&l.path)).collect()).unwrap_or_default();
        let entry = d.entry.native_lib.as_ref().map(|n| match d.native_path(abi, n) {
            Some(p) => abs(p),
            None => {
                let f = if n.ends_with(".so") { n.clone() } else { format!("{n}.so") };
                abs(&format!("{}/{abi}/{f}", d.native_dir.as_deref().unwrap_or("lib")))
            }
        });
        if let Some(e) = &entry {
            if !libs.contains(e) {
                libs.push(e.clone());
            }
        }
        let names: Vec<Value> = d
            .native
            .get(abi)
            .map(|v| v.iter().map(|l| json!({"name": l.name, "path": abs(&l.path)})).collect())
            .unwrap_or_default();
        Ok(json!({
            "app": d.app, "version": d.version, "host_api_min": d.host_api_min, "native": names,
            "libs": libs, "entry_lib": entry, "symbol": d.entry.symbol,
            "dex_class": d.entry.dex_class, "dex": d.dex.iter().map(|p| abs(p)).collect::<Vec<_>>(),
            "transparent": d.entry.transparent,
        }))
    }

    pub fn verify_file(path: &str, sha256: &str, size: u64) -> Result<Value> {
        crate::hash::verify_file(std::path::Path::new(path), sha256, size).map(|_| Value::Null)
    }

    #[cfg(feature = "transport")]
    pub fn update_http(root: &str, config_json: &str, manifest_url: &str, random16: &[u8], now: i64) -> Result<Value> {
        let cfg: Config = serde_json::from_str(config_json).map_err(|e| Error::Usage(format!("config: {e}")))?;
        let rep = crate::transport::update_once(Store::open(root)?, cfg, manifest_url, r16(random16)?, now)?;
        Ok(serde_json::to_value(rep).unwrap())
    }
}

unsafe fn s<'a>(p: *const c_char) -> Result<&'a str> {
    if p.is_null() {
        return Err(Error::Usage("null string".into()));
    }
    CStr::from_ptr(p).to_str().map_err(|_| Error::Usage("string is not UTF-8".into()))
}

unsafe fn opt_s(p: *const c_char) -> Result<Option<String>> {
    if p.is_null() {
        Ok(None)
    } else {
        s(p).map(|x| Some(x.to_string()))
    }
}

unsafe fn bytes<'a>(p: *const u8, n: usize) -> &'a [u8] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(p, n)
    }
}

fn out(sv: String) -> *mut c_char {
    CString::new(sv)
        .unwrap_or_else(|_| CString::new("{\"error\":{\"code\":\"usage\",\"message\":\"NUL in output\"}}").unwrap())
        .into_raw()
}

/// Free a string returned by any `andysu_*` function.
#[no_mangle]
pub unsafe extern "C" fn andysu_free(p: *mut c_char) {
    if !p.is_null() {
        drop(CString::from_raw(p));
    }
}

/// Library version, static string (do not free).
#[no_mangle]
pub extern "C" fn andysu_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// `{"ok": <session handle>}`. `random16` (16 bytes) seeds the install id on first run.
#[no_mangle]
pub unsafe extern "C" fn andysu_session_new(
    root: *const c_char,
    config_json: *const c_char,
    random16: *const u8,
) -> *mut c_char {
    out(api::guarded(|| api::session_new(s(root)?, s(config_json)?, bytes(random16, 16))))
}

#[no_mangle]
pub extern "C" fn andysu_session_free(h: u64) {
    api::session_free(h)
}

/// Verify + parse + select. `etag` may be NULL.
#[no_mangle]
pub unsafe extern "C" fn andysu_session_check(
    h: u64,
    manifest_url: *const c_char,
    manifest: *const u8,
    manifest_len: usize,
    sig: *const u8,
    sig_len: usize,
    etag: *const c_char,
    now: i64,
) -> *mut c_char {
    out(api::guarded(|| {
        api::check(h, s(manifest_url)?, bytes(manifest, manifest_len), bytes(sig, sig_len), opt_s(etag)?, now)
    }))
}

/// After HTTP 304: re-verify the cached manifest.
#[no_mangle]
pub extern "C" fn andysu_session_check_cached(h: u64, now: i64) -> *mut c_char {
    out(api::guarded(|| api::check_cached(h, now)))
}

#[no_mangle]
pub extern "C" fn andysu_session_plan(h: u64) -> *mut c_char {
    out(api::guarded(|| api::plan(h)))
}

#[no_mangle]
pub unsafe extern "C" fn andysu_session_complete(h: u64, key: *const c_char) -> *mut c_char {
    out(api::guarded(|| api::complete(h, s(key)?)))
}

#[no_mangle]
pub unsafe extern "C" fn andysu_session_skip_delta(h: u64, key: *const c_char) -> *mut c_char {
    out(api::guarded(|| api::skip_delta(h, s(key)?)))
}

#[no_mangle]
pub extern "C" fn andysu_session_stage(h: u64) -> *mut c_char {
    out(api::guarded(|| api::stage(h)))
}

/// Cold-start decision; persists before returning. `{"ok":{"slot":..,"slot_dir":..,"in_trial":..,"events":[..]}}`
#[no_mangle]
pub unsafe extern "C" fn andysu_launch(
    root: *const c_char,
    random16: *const u8,
    now: i64,
    apply_pending: bool,
    max_attempts: u32,
) -> *mut c_char {
    out(api::guarded(|| api::launch(s(root)?, bytes(random16, 16), now, apply_pending, max_attempts)))
}

#[no_mangle]
pub unsafe extern "C" fn andysu_mark_healthy(root: *const c_char) -> *mut c_char {
    out(api::guarded(|| api::mark_healthy(s(root)?)))
}

#[no_mangle]
pub unsafe extern "C" fn andysu_report_failure(root: *const c_char, reason: *const c_char) -> *mut c_char {
    out(api::guarded(|| api::report_failure(s(root)?, s(reason)?)))
}

#[no_mangle]
pub unsafe extern "C" fn andysu_begin_live(root: *const c_char, now: i64) -> *mut c_char {
    out(api::guarded(|| api::begin_live(s(root)?, now)))
}

/// Recovery: forget installed slots and boot the payload built into the host.
#[no_mangle]
pub unsafe extern "C" fn andysu_reset(root: *const c_char, reason: *const c_char) -> *mut c_char {
    out(api::guarded(|| api::reset(s(root)?, s(reason)?)))
}

#[no_mangle]
pub unsafe extern "C" fn andysu_gc(root: *const c_char, low_storage: bool) -> *mut c_char {
    out(api::guarded(|| api::gc(s(root)?, low_storage, &[])))
}

#[no_mangle]
pub unsafe extern "C" fn andysu_state(root: *const c_char) -> *mut c_char {
    out(api::guarded(|| api::state(s(root)?)))
}

/// Libraries to load (in order), entry library and symbol, dex path for a slot directory.
#[no_mangle]
pub unsafe extern "C" fn andysu_load_plan(slot_dir: *const c_char, abi: *const c_char) -> *mut c_char {
    out(api::guarded(|| api::load_plan(s(slot_dir)?, s(abi)?)))
}

/// Path a native signal handler should create to report a crash (see `Store::crash_marker_path`).
#[no_mangle]
pub unsafe extern "C" fn andysu_crash_marker_path(root: *const c_char) -> *mut c_char {
    out(api::guarded(|| api::crash_marker_path(s(root)?)))
}

/// One complete check/download/stage cycle over HTTP(S) or file:// (transport feature).
#[cfg(feature = "transport")]
#[no_mangle]
pub unsafe extern "C" fn andysu_update_http(
    root: *const c_char,
    config_json: *const c_char,
    manifest_url: *const c_char,
    random16: *const u8,
    now: i64,
) -> *mut c_char {
    out(api::guarded(|| api::update_http(s(root)?, s(config_json)?, s(manifest_url)?, bytes(random16, 16), now)))
}
