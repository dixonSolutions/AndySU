//! JNI entry points for `com.andysu.host.Core` (Android host). Every method returns the same
//! JSON envelope as the C ABI. Byte arrays and strings are copied; no JNI references are kept.

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;

use crate::ffi::api;
use crate::Error;

fn st(env: &mut JNIEnv, s: &JString) -> crate::Result<String> {
    env.get_string(s).map(Into::into).map_err(|e| Error::Usage(format!("jni string: {e}")))
}

fn opt_st(env: &mut JNIEnv, s: &JString) -> crate::Result<Option<String>> {
    if s.is_null() {
        Ok(None)
    } else {
        st(env, s).map(Some)
    }
}

fn by(env: &mut JNIEnv, b: &JByteArray) -> crate::Result<Vec<u8>> {
    if b.is_null() {
        return Ok(vec![]);
    }
    env.convert_byte_array(b).map_err(|e| Error::Usage(format!("jni bytes: {e}")))
}

fn ret(env: &mut JNIEnv, s: String) -> jstring {
    env.new_string(s).map(|j| j.into_raw()).unwrap_or(std::ptr::null_mut())
}

macro_rules! jfn {
    ($name:ident ( $env:ident $(, $arg:ident : $ty:ty)* ) $body:block) => {
        #[no_mangle]
        pub extern "system" fn $name<'l>(mut $env: JNIEnv<'l>, _c: JClass<'l> $(, $arg: $ty)*) -> jstring {
            let r = api::guarded(|| $body);
            ret(&mut $env, r)
        }
    };
}

jfn!(Java_com_andysu_host_Core_version(env) { Ok(serde_json::json!(crate::VERSION)) });

jfn!(Java_com_andysu_host_Core_sessionNew(env, root: JString<'l>, config: JString<'l>, random16: JByteArray<'l>) {
    let (r, c, b) = (st(&mut env, &root)?, st(&mut env, &config)?, by(&mut env, &random16)?);
    api::session_new(&r, &c, &b)
});

jfn!(Java_com_andysu_host_Core_sessionFree(env, h: jlong) {
    api::session_free(h as u64);
    Ok(serde_json::Value::Null)
});

jfn!(Java_com_andysu_host_Core_sessionCheck(env, h: jlong, url: JString<'l>, manifest: JByteArray<'l>, sig: JByteArray<'l>, etag: JString<'l>, now: jlong) {
    let (u, m, s, e) = (st(&mut env, &url)?, by(&mut env, &manifest)?, by(&mut env, &sig)?, opt_st(&mut env, &etag)?);
    api::check(h as u64, &u, &m, &s, e, now)
});

jfn!(Java_com_andysu_host_Core_sessionCheckCached(env, h: jlong, now: jlong) { api::check_cached(h as u64, now) });

jfn!(Java_com_andysu_host_Core_sessionPlan(env, h: jlong) { api::plan(h as u64) });

jfn!(Java_com_andysu_host_Core_sessionComplete(env, h: jlong, key: JString<'l>) {
    let k = st(&mut env, &key)?;
    api::complete(h as u64, &k)
});

jfn!(Java_com_andysu_host_Core_sessionSkipDelta(env, h: jlong, key: JString<'l>) {
    let k = st(&mut env, &key)?;
    api::skip_delta(h as u64, &k)
});

jfn!(Java_com_andysu_host_Core_sessionStage(env, h: jlong) { api::stage(h as u64) });

jfn!(Java_com_andysu_host_Core_etag(env, root: JString<'l>) {
    let r = st(&mut env, &root)?;
    api::etag(&r)
});

jfn!(Java_com_andysu_host_Core_launch(env, root: JString<'l>, random16: JByteArray<'l>, now: jlong, apply: jboolean, max: jint) {
    let (r, b) = (st(&mut env, &root)?, by(&mut env, &random16)?);
    api::launch(&r, &b, now, apply != 0, max.max(0) as u32)
});

jfn!(Java_com_andysu_host_Core_markHealthy(env, root: JString<'l>) {
    let r = st(&mut env, &root)?;
    api::mark_healthy(&r)
});

jfn!(Java_com_andysu_host_Core_reportFailure(env, root: JString<'l>, reason: JString<'l>) {
    let (r, why) = (st(&mut env, &root)?, st(&mut env, &reason)?);
    api::report_failure(&r, &why)
});

jfn!(Java_com_andysu_host_Core_beginLive(env, root: JString<'l>, now: jlong) {
    let r = st(&mut env, &root)?;
    api::begin_live(&r, now)
});

jfn!(Java_com_andysu_host_Core_gc(env, root: JString<'l>, low: jboolean) {
    let r = st(&mut env, &root)?;
    api::gc(&r, low != 0, &[])
});

jfn!(Java_com_andysu_host_Core_state(env, root: JString<'l>) {
    let r = st(&mut env, &root)?;
    api::state(&r)
});

jfn!(Java_com_andysu_host_Core_crashMarkerPath(env, root: JString<'l>) {
    let r = st(&mut env, &root)?;
    api::crash_marker_path(&r)
});

jfn!(Java_com_andysu_host_Core_verifyFile(env, path: JString<'l>, sha: JString<'l>, size: jlong) {
    let (p, h) = (st(&mut env, &path)?, st(&mut env, &sha)?);
    api::verify_file(&p, &h, size as u64)
});
