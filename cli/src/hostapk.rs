//! Build the frozen host APK from any finished APK (`andysu split --host-template ...`).
//!
//! The original APK keeps its identity (package, version, label, icon, resources, signing key)
//! but its code becomes the AndySU host from a prebuilt template (`classes*.dex` +
//! `lib/<abi>/libandysu_*.so`). Its own components are mapped onto host stubs, keeping their
//! attributes and intent filters; everything they ran is in the payload.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::axml::{self, attr, Attr, Element, Node, Value, TYPE_INT_BOOLEAN, TYPE_REFERENCE};
use crate::{read, R};

const THEME_TRANSLUCENT_NO_TITLE: u32 = 0x0103_0010;
const HOST_PERMISSIONS: [&str; 3] = [
    "android.permission.INTERNET",
    "android.permission.ACCESS_NETWORK_STATE",
    "android.permission.RECEIVE_BOOT_COMPLETED",
];
const STUB_ACTIVITIES: [&str; 4] = [
    "com.andysu.host.HostActivity",
    "com.andysu.host.StubActivity1",
    "com.andysu.host.StubActivity2",
    "com.andysu.host.StubActivity3",
];
const STUB_SERVICES: [&str; 2] = ["com.andysu.host.StubService0", "com.andysu.host.StubService1"];
const STUB_RECEIVERS: [&str; 2] = ["com.andysu.host.StubReceiver0", "com.andysu.host.StubReceiver1"];

pub struct HostOptions {
    pub template: PathBuf,
    pub config: PathBuf,
    pub factory: PathBuf,
    pub out: PathBuf,
    pub keystore: Option<PathBuf>,
    pub ks_pass: Option<String>,
    pub key_alias: Option<String>,
    pub cleartext: bool,
    pub transparent: bool,
    pub payload_has_assets: bool,
}

#[derive(serde::Serialize, Debug, Default)]
pub struct HostReport {
    pub apk: String,
    pub signed: bool,
    /// original component -> stub it now runs as
    pub stubs: BTreeMap<String, String>,
    pub dropped: Vec<String>,
    pub abis: Vec<String>,
    pub warnings: Vec<String>,
}

fn b(v: bool) -> Value {
    Value::Typed { dtype: TYPE_INT_BOOLEAN, data: if v { u32::MAX } else { 0 } }
}

fn s(v: &str) -> Value {
    Value::Str(v.into())
}

fn is_launcher(e: &Element) -> bool {
    e.elements().filter(|f| f.name == "intent-filter").any(|f| {
        let has = |tag: &str, v: &str| f.elements().any(|x| x.name == tag && x.android_name() == Some(v));
        has("action", "android.intent.action.MAIN") && has("category", "android.intent.category.LAUNCHER")
    })
}

fn full_name(pkg: &str, n: &str) -> String {
    if n.starts_with('.') {
        format!("{pkg}{n}")
    } else if !n.contains('.') {
        format!("{pkg}.{n}")
    } else {
        n.to_string()
    }
}

/// Rewrite the manifest; returns (new manifest bytes, stub map, dropped components).
/// Transparent mode: keep every component; the host's AppComponentFactory gives the platform the
/// payload's class loader, and an init provider starts AndySU before Application.onCreate.
pub fn transform_transparent(orig: &[u8], rep: &mut HostReport, cleartext: bool, needs_assets: bool) -> R<Vec<u8>> {
    let mut doc = axml::parse(orig)?;
    let pkg = doc
        .root
        .attrs
        .iter()
        .find(|a| a.name == "package")
        .and_then(|a| a.str())
        .ok_or("manifest has no package")?
        .to_string();
    let have: Vec<String> = doc
        .root
        .elements()
        .filter(|e| e.name == "uses-permission")
        .filter_map(|e| e.android_name().map(String::from))
        .collect();
    let app_pos = doc
        .root
        .children
        .iter()
        .position(|n| matches!(n, Node::Element(e) if e.name == "application"))
        .ok_or("manifest has no <application>")?;
    for (i, p) in HOST_PERMISSIONS.iter().filter(|p| !have.iter().any(|h| h == *p)).enumerate() {
        doc.root.children.insert(
            app_pos + i,
            Node::Element(Element::new("uses-permission", vec![Attr::android("name", attr::NAME, s(p))])),
        );
    }
    // AppComponentFactory needs Android 9; payload assets through ResourcesLoader need Android 11.
    const MIN_SDK: u32 = 0x0101_020c;
    let need = if needs_assets { 30 } else { 28 };
    if let Some(us) = doc.root.child_mut("uses-sdk") {
        let cur = match us.attr(MIN_SDK).map(|a| &a.value) {
            Some(Value::Typed { data, .. }) => *data,
            Some(Value::Str(x)) => x.parse().unwrap_or(1),
            None => 1,
        };
        if cur < need {
            us.set(Attr::android("minSdkVersion", MIN_SDK, Value::Typed { dtype: axml::TYPE_INT_DEC, data: need }));
            rep.warnings.push(format!("transparent mode raises minSdkVersion from {cur} to {need}"));
        }
    }
    let app = doc.root.child_mut("application").unwrap();
    if let Some(f) = app.attr(attr::APP_COMPONENT_FACTORY).and_then(|a| a.str()) {
        rep.warnings.push(format!("replacing the app's AppComponentFactory {f}"));
    }
    app.set(Attr::android(
        "appComponentFactory",
        attr::APP_COMPONENT_FACTORY,
        s("com.andysu.host.AndySUComponentFactory"),
    ));
    app.remove(attr::HAS_CODE);
    // Libraries are stored uncompressed and 16 KB aligned and mapped straight from the APK.
    app.set(Attr::android("extractNativeLibs", attr::EXTRACT_NATIVE_LIBS, b(false)));
    if cleartext {
        const USES_CLEARTEXT_TRAFFIC: u32 = 0x0101_04ec;
        app.set(Attr::android("usesCleartextTraffic", USES_CLEARTEXT_TRAFFIC, b(true)));
        rep.warnings.push("--cleartext: plain HTTP allowed (testing only)".into());
    }
    const INIT_ORDER: u32 = 0x0101_001a;
    app.push(Element::new(
        "provider",
        vec![
            Attr::android("name", attr::NAME, s("com.andysu.host.InitProvider")),
            Attr::android("exported", attr::EXPORTED, b(false)),
            Attr::android("authorities", attr::AUTHORITIES, s(&format!("{pkg}.andysu.init"))),
            Attr::android("initOrder", INIT_ORDER, Value::Typed { dtype: axml::TYPE_INT_DEC, data: 10_000 }),
        ],
    ));
    push_host_components(app, &pkg);
    Ok(axml::write(&doc))
}

fn push_host_components(app: &mut Element, pkg: &str) {
    app.push(Element::new(
        "activity",
        vec![
            Attr::android("name", attr::NAME, s("com.andysu.host.PhoenixActivity")),
            Attr::android("process", attr::PROCESS, s(":andysu_phoenix")),
            Attr::android(
                "theme",
                attr::THEME,
                Value::Typed { dtype: TYPE_REFERENCE, data: THEME_TRANSLUCENT_NO_TITLE },
            ),
            Attr::android("excludeFromRecents", attr::EXCLUDE_FROM_RECENTS, b(true)),
            Attr::android("noHistory", attr::NO_HISTORY, b(true)),
            Attr::android("exported", attr::EXPORTED, b(false)),
        ],
    ));
    app.push(Element::new(
        "service",
        vec![
            Attr::android("name", attr::NAME, s("com.andysu.host.UpdateJobService")),
            Attr::android("permission", attr::PERMISSION, s("android.permission.BIND_JOB_SERVICE")),
            Attr::android("exported", attr::EXPORTED, b(false)),
        ],
    ));
    let mut recv = Element::new(
        "receiver",
        vec![
            Attr::android("name", attr::NAME, s("com.andysu.host.ShellInstallReceiver")),
            Attr::android("exported", attr::EXPORTED, b(false)),
        ],
    );
    let mut f = Element::new("intent-filter", vec![]);
    f.push(Element::new("action", vec![Attr::android("name", attr::NAME, s("com.andysu.host.SHELL_INSTALL_STATUS"))]));
    recv.push(f);
    app.push(recv);
    app.push(Element::new(
        "provider",
        vec![
            Attr::android("name", attr::NAME, s("com.andysu.host.StubFileProvider")),
            Attr::android("exported", attr::EXPORTED, b(false)),
            Attr::android("authorities", attr::AUTHORITIES, s(&format!("{pkg}.andysu.files"))),
            Attr::android("grantUriPermissions", attr::GRANT_URI_PERMISSIONS, b(true)),
        ],
    ));
}

pub fn transform_manifest(orig: &[u8], rep: &mut HostReport, cleartext: bool) -> R<Vec<u8>> {
    let mut doc = axml::parse(orig)?;
    let pkg = doc
        .root
        .attrs
        .iter()
        .find(|a| a.name == "package")
        .and_then(|a| a.str())
        .ok_or("manifest has no package")?
        .to_string();
    // Permissions: add the host's normal ones.
    let have: Vec<String> = doc
        .root
        .elements()
        .filter(|e| e.name == "uses-permission")
        .filter_map(|e| e.android_name().map(String::from))
        .collect();
    let app_pos = doc
        .root
        .children
        .iter()
        .position(|n| matches!(n, Node::Element(e) if e.name == "application"))
        .ok_or("manifest has no <application>")?;
    for (i, p) in HOST_PERMISSIONS.iter().filter(|p| !have.iter().any(|h| h == *p)).enumerate() {
        doc.root.children.insert(
            app_pos + i,
            Node::Element(Element::new("uses-permission", vec![Attr::android("name", attr::NAME, s(p))])),
        );
    }
    let app = doc.root.child_mut("application").unwrap();
    app.set(Attr::android("name", attr::NAME, s("com.andysu.host.AndySUApplication")));
    app.remove(attr::APP_COMPONENT_FACTORY);
    app.remove(attr::HAS_CODE);
    // Libraries are stored uncompressed and 16 KB aligned and mapped straight from the APK.
    app.set(Attr::android("extractNativeLibs", attr::EXTRACT_NATIVE_LIBS, b(false)));
    if cleartext {
        const USES_CLEARTEXT_TRAFFIC: u32 = 0x0101_04ec;
        app.set(Attr::android("usesCleartextTraffic", USES_CLEARTEXT_TRAFFIC, b(true)));
        rep.warnings.push("--cleartext: plain HTTP allowed (testing only)".into());
    }

    // Activities: launcher first onto HostActivity, the rest onto stubs 1..3.
    let mut acts: Vec<usize> = app
        .children
        .iter()
        .enumerate()
        .filter(|(_, n)| matches!(n, Node::Element(e) if e.name == "activity"))
        .map(|(i, _)| i)
        .collect();
    acts.sort_by_key(|&i| if let Node::Element(e) = &app.children[i] { !is_launcher(e) } else { true });
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    let mut drop_idx = vec![];
    let (mut ai, mut si, mut ri) = (0usize, 0usize, 0usize);
    for &i in &acts {
        let Node::Element(e) = &mut app.children[i] else { continue };
        let old = full_name(&pkg, e.android_name().unwrap_or(""));
        if ai >= STUB_ACTIVITIES.len() {
            return Err(format!(
                "the app has more than {} activities; this needs more stub activities in the host",
                STUB_ACTIVITIES.len()
            ));
        }
        e.set(Attr::android("name", attr::NAME, s(STUB_ACTIVITIES[ai])));
        renamed.insert(old, STUB_ACTIVITIES[ai].into());
        ai += 1;
    }
    for (i, n) in app.children.iter_mut().enumerate() {
        let Node::Element(e) = n else { continue };
        let old = full_name(&pkg, e.android_name().unwrap_or(""));
        match e.name.as_str() {
            "service" | "receiver" => {
                let (stubs, k): (&[&str], &mut usize) =
                    if e.name == "service" { (&STUB_SERVICES, &mut si) } else { (&STUB_RECEIVERS, &mut ri) };
                if *k < stubs.len() {
                    e.set(Attr::android("name", attr::NAME, s(stubs[*k])));
                    renamed.insert(old, stubs[*k].into());
                    *k += 1;
                } else {
                    drop_idx.push(i);
                    rep.dropped.push(old);
                }
            }
            "provider" => {
                drop_idx.push(i);
                rep.dropped.push(old);
            }
            "activity-alias" => {
                // targetActivity must follow the renamed activity.
                const TARGET_ACTIVITY: u32 = 0x0101_0202;
                if let Some(t) = e.attr(TARGET_ACTIVITY).and_then(|a| a.str()).map(|t| full_name(&pkg, t)) {
                    if let Some(new) = renamed.get(&t) {
                        let new = new.clone();
                        e.set(Attr::android("targetActivity", TARGET_ACTIVITY, s(&new)));
                    }
                }
            }
            _ => {}
        }
    }
    for i in drop_idx.into_iter().rev() {
        app.children.remove(i);
    }
    if !rep.dropped.is_empty() {
        rep.warnings.push(format!(
            "{} component(s) cannot run from a host stub and were left out: {}. Payload code can do their work from Payload.start().",
            rep.dropped.len(),
            rep.dropped.join(", ")
        ));
    }
    // Stubs the original did not use, and the host's own components. An app without a launcher
    // activity gets one: the host's.
    for (k, st) in STUB_ACTIVITIES.iter().enumerate().skip(ai) {
        let launcher = k == 0;
        let mut e = Element::new(
            "activity",
            vec![Attr::android("name", attr::NAME, s(st)), Attr::android("exported", attr::EXPORTED, b(launcher))],
        );
        if launcher {
            let mut f = Element::new("intent-filter", vec![]);
            f.push(Element::new("action", vec![Attr::android("name", attr::NAME, s("android.intent.action.MAIN"))]));
            f.push(Element::new(
                "category",
                vec![Attr::android("name", attr::NAME, s("android.intent.category.LAUNCHER"))],
            ));
            e.push(f);
        }
        app.push(e);
    }
    for st in STUB_SERVICES.iter().skip(si) {
        app.push(Element::new(
            "service",
            vec![Attr::android("name", attr::NAME, s(st)), Attr::android("exported", attr::EXPORTED, b(false))],
        ));
    }
    for st in STUB_RECEIVERS.iter().skip(ri) {
        app.push(Element::new(
            "receiver",
            vec![Attr::android("name", attr::NAME, s(st)), Attr::android("exported", attr::EXPORTED, b(false))],
        ));
    }
    push_host_components(app, &pkg);
    rep.stubs = renamed;
    Ok(axml::write(&doc))
}

fn is_signature(n: &str) -> bool {
    n.starts_with("META-INF/") && [".SF", ".RSA", ".EC", ".DSA", "MANIFEST.MF"].iter().any(|x| n.ends_with(x))
}

fn apksigner() -> Option<PathBuf> {
    let sdk = std::env::var("ANDROID_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Android/Sdk")))?;
    let mut v: Vec<PathBuf> = std::fs::read_dir(sdk.join("build-tools"))
        .ok()?
        .flatten()
        .map(|e| e.path().join("apksigner"))
        .filter(|p| p.exists())
        .collect();
    v.sort();
    v.pop()
}

pub fn build(orig_apk: &Path, o: &HostOptions) -> R<HostReport> {
    let mut rep = HostReport::default();
    let mut z =
        zip::ZipArchive::new(std::fs::File::open(orig_apk).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut tpl =
        zip::ZipArchive::new(std::fs::File::open(&o.template).map_err(|e| format!("{}: {e}", o.template.display()))?)
            .map_err(|e| e.to_string())?;
    let mut manifest = vec![];
    z.by_name("AndroidManifest.xml")
        .map_err(|e| e.to_string())?
        .read_to_end(&mut manifest)
        .map_err(|e| e.to_string())?;
    let new_manifest = if o.transparent {
        transform_transparent(&manifest, &mut rep, o.cleartext, o.payload_has_assets)?
    } else {
        transform_manifest(&manifest, &mut rep, o.cleartext)?
    };

    let orig_abis: Vec<String> = {
        let mut v: Vec<String> = z
            .file_names()
            .filter_map(|n| n.strip_prefix("lib/"))
            .filter_map(|r| r.split('/').next())
            .map(String::from)
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let tpl_names: Vec<String> = tpl.file_names().map(String::from).collect();
    let tpl_abis: Vec<String> = {
        let mut v: Vec<String> = tpl_names
            .iter()
            .filter_map(|n| n.strip_prefix("lib/"))
            .filter_map(|r| r.split('/').next())
            .map(String::from)
            .collect();
        v.sort();
        v.dedup();
        v
    };
    rep.abis = if orig_abis.is_empty() {
        tpl_abis.clone()
    } else {
        orig_abis.iter().filter(|a| tpl_abis.contains(a)).cloned().collect()
    };
    for a in &orig_abis {
        if !tpl_abis.contains(a) {
            rep.warnings.push(format!("the host template has no {a} build; that ABI is left out"));
        }
    }

    let unsigned = o.out.with_extension("unsigned.apk");
    {
        let f = std::fs::File::create(&unsigned).map_err(|e| e.to_string())?;
        let mut w = zip::ZipWriter::new(f);
        let deflate = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let stored4 = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .with_alignment(4);
        // Uncompressed and 16 KB aligned: mapped from the APK directly (extractNativeLibs=false),
        // as Android 15+ devices with 16 KB pages require.
        let stored16k = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .with_alignment(16384);
        w.start_file("AndroidManifest.xml", deflate).map_err(|e| e.to_string())?;
        w.write_all(&new_manifest).map_err(|e| e.to_string())?;
        for i in 0..z.len() {
            let e = z.by_index_raw(i).map_err(|e| e.to_string())?;
            let n = e.name().to_string();
            drop(e);
            if n == "resources.arsc" {
                // Android 11+ requires it uncompressed and 4-byte aligned.
                let mut data = vec![];
                z.by_index(i).map_err(|e| e.to_string())?.read_to_end(&mut data).map_err(|e| e.to_string())?;
                w.start_file(n.as_str(), stored4).map_err(|e| e.to_string())?;
                w.write_all(&data).map_err(|e| e.to_string())?;
            } else if n.starts_with("res/") && !n.ends_with('/') && !is_signature(&n) {
                w.raw_copy_file(z.by_index_raw(i).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            }
        }
        for n in &tpl_names {
            let keep = (n.starts_with("classes") && n.ends_with(".dex"))
                || n.strip_prefix("lib/").is_some_and(|r| rep.abis.iter().any(|a| r.starts_with(&format!("{a}/"))));
            if keep && !n.ends_with('/') {
                let mut data = vec![];
                tpl.by_name(n).map_err(|e| e.to_string())?.read_to_end(&mut data).map_err(|e| e.to_string())?;
                w.start_file(n.as_str(), if n.ends_with(".so") { stored16k } else { deflate })
                    .map_err(|e| e.to_string())?;
                w.write_all(&data).map_err(|e| e.to_string())?;
            }
        }
        w.start_file("assets/andysu/config.json", deflate).map_err(|e| e.to_string())?;
        w.write_all(&read(&o.config)?).map_err(|e| e.to_string())?;
        for rel in crate::pack::walk(&o.factory)? {
            w.start_file(format!("assets/andysu-factory/{rel}"), deflate).map_err(|e| e.to_string())?;
            w.write_all(&read(&o.factory.join(&rel))?).map_err(|e| e.to_string())?;
        }
        w.finish().map_err(|e| e.to_string())?;
    }
    match &o.keystore {
        Some(ks) => {
            let signer = apksigner().ok_or("apksigner not found (set ANDROID_HOME)")?;
            let mut c = Command::new(signer);
            c.args(["sign", "--ks"]).arg(ks);
            if let Some(p) = &o.ks_pass {
                c.args(["--ks-pass", p]);
            }
            if let Some(a) = &o.key_alias {
                c.args(["--ks-key-alias", a]);
            }
            c.arg("--out").arg(&o.out).arg(&unsigned);
            let r = c.output().map_err(|e| e.to_string())?;
            if !r.status.success() {
                return Err(format!("apksigner failed: {}", String::from_utf8_lossy(&r.stderr)));
            }
            let _ = std::fs::remove_file(&unsigned);
            rep.signed = true;
            rep.apk = o.out.display().to_string();
        }
        None => {
            rep.apk = unsigned.display().to_string();
            rep.warnings
                .push("no --keystore: the host APK is unsigned; sign it with the app's own key (apksigner)".into());
        }
    }
    Ok(rep)
}
