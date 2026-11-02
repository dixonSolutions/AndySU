//! `andysu split app.apk`: a finished APK becomes a payload directory (and, with the Gradle
//! plugin or `--host`, a frozen host APK; see docs).
//!
//! The payload gets every `classes*.dex` (dex/), every `lib/<abi>/*.so` (lib/<abi>/), the assets
//! (assets/), Java resources (dex/resources.jar), and a descriptor whose `needs` list what the
//! original manifest asked for, so `andysu pack --host` can refuse payloads the host cannot run.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use andysu_core::payload::{Descriptor, Entry, Needs};

use crate::apk::Apk;
use crate::{write, R};

#[derive(clap::Args, Clone, Debug)]
pub struct SplitArgs {
    /// The finished APK.
    pub apk: PathBuf,
    /// Output directory (receives payload/ and andysu-host.json).
    #[arg(long, short)]
    pub out: PathBuf,
    /// Application id (default: the APK's package).
    #[arg(long)]
    pub app: Option<String>,
    /// Payload version (default: the APK's versionName).
    #[arg(long)]
    pub version: Option<String>,
    /// JVM entry class implementing com.andysu.api.Payload.
    #[arg(long)]
    pub entry_class: Option<String>,
    /// Native entry, LIB:SYMBOL (e.g. libgame:andysu_payload_v1).
    #[arg(long)]
    pub entry_native: Option<String>,
    /// Host API level the payload needs.
    #[arg(long, default_value_t = 1)]
    pub host_api: u32,
    /// Keep only these ABIs (default: all).
    #[arg(long)]
    pub abi: Vec<String>,
}

#[derive(serde::Serialize, Debug, Default)]
pub struct SplitReport {
    pub payload: String,
    pub app: String,
    pub version: String,
    pub dex: Vec<String>,
    pub native: BTreeMap<String, Vec<String>>,
    pub assets: usize,
    pub java_resources: usize,
    pub manifest: crate::apk::ManifestInfo,
}

fn is_java_resource(n: &str) -> bool {
    !(n.ends_with('/')
        || n == "AndroidManifest.xml"
        || n == "resources.arsc"
        || n.starts_with("res/")
        || n.starts_with("assets/")
        || n.starts_with("lib/")
        || n.starts_with("META-INF/")
            && (n.ends_with(".SF")
                || n.ends_with(".RSA")
                || n.ends_with(".EC")
                || n.ends_with(".DSA")
                || n.ends_with("MANIFEST.MF"))
        || (n.starts_with("classes") && n.ends_with(".dex") && !n.contains('/'))
        || n.starts_with("stamp-cert-sha256"))
}

pub fn split(a: &SplitArgs) -> R<SplitReport> {
    let mut apk = Apk::open(&a.apk)?;
    let mi = apk.manifest()?;
    let payload = a.out.join("payload");
    if payload.exists() && std::fs::read_dir(&payload).map(|mut d| d.next().is_some()).unwrap_or(false) {
        return Err(format!("{} is not empty", payload.display()));
    }
    let mut rep = SplitReport { payload: payload.display().to_string(), ..Default::default() };
    let mut javares: Vec<(String, Vec<u8>)> = vec![];
    for n in apk.names() {
        if n.starts_with("classes") && n.ends_with(".dex") && !n.contains('/') {
            write(&payload.join("dex").join(&n), &apk.read(&n)?)?;
            rep.dex.push(format!("dex/{n}"));
        } else if let Some(rest) = n.strip_prefix("lib/") {
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() == 2
                && parts[1].ends_with(".so")
                && (a.abi.is_empty() || a.abi.iter().any(|x| x == parts[0]))
            {
                write(&payload.join(&n), &apk.read(&n)?)?;
                rep.native.entry(parts[0].to_string()).or_default().push(parts[1].to_string());
            }
        } else if let Some(rest) = n.strip_prefix("assets/") {
            if rest.is_empty() || n.ends_with('/') || rest.starts_with("andysu/") || rest.starts_with("andysu-factory/")
            {
                continue;
            }
            andysu_core::manifest::check_rel_path(&n).map_err(|e| e.to_string())?;
            write(&payload.join(&n), &apk.read(&n)?)?;
            rep.assets += 1;
        } else if is_java_resource(&n) {
            andysu_core::manifest::check_rel_path(&n).map_err(|e| e.to_string())?;
            javares.push((n.clone(), apk.read(&n)?));
        }
    }
    // dex order: classes.dex, classes2.dex, ...
    rep.dex.sort_by_key(|p| p.trim_start_matches("dex/classes").trim_end_matches(".dex").parse::<u32>().unwrap_or(1));
    let mut dex = rep.dex.clone();
    if !javares.is_empty() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default().last_modified_time(zip::DateTime::default());
            for (n, d) in &javares {
                w.start_file(n.as_str(), o).map_err(|e| e.to_string())?;
                w.write_all(d).map_err(|e| e.to_string())?;
            }
            w.finish().map_err(|e| e.to_string())?;
        }
        write(&payload.join("dex/resources.jar"), &buf.into_inner())?;
        dex.push("dex/resources.jar".into());
        rep.java_resources = javares.len();
    }
    let entry = match (&a.entry_class, &a.entry_native) {
        (Some(c), None) => Entry { dex_class: Some(c.clone()), ..Default::default() },
        (None, Some(n)) => {
            let (lib, sym) = n.split_once(':').ok_or("--entry-native wants LIB:SYMBOL")?;
            Entry {
                native_lib: Some(lib.trim_end_matches(".so").into()),
                symbol: Some(sym.into()),
                ..Default::default()
            }
        }
        _ => return Err("give exactly one of --entry-class or --entry-native".into()),
    };
    let app = a.app.clone().unwrap_or_else(|| mi.package.clone());
    let version =
        a.version.clone().or_else(|| mi.version_name.clone()).ok_or("no --version and no versionName in the APK")?;
    let own =
        |c: &str| !c.starts_with("com.andysu.host.") && !c.starts_with("androidx.") && !c.starts_with("com.google.");
    let desc = Descriptor {
        schema: 1,
        app: app.clone(),
        version: version.clone(),
        host_api_min: a.host_api,
        entry,
        dex,
        native_dir: None,
        needs: Needs {
            permissions: mi.permissions.clone(),
            activities: mi.activities.iter().filter(|c| own(c)).count() as u32,
            services: mi.services.iter().filter(|c| own(c)).count() as u32,
            receivers: mi.receivers.iter().filter(|c| own(c)).count() as u32,
            providers: mi.providers.iter().filter(|c| own(c)).count() as u32,
            features: mi.features.clone(),
        },
        native: BTreeMap::new(),
    };
    desc.validate().map_err(|e| e.to_string())?;
    let mut dj = serde_json::to_vec_pretty(&desc).unwrap();
    dj.push(b'\n');
    write(&payload.join("andysu-payload.json"), &dj)?;
    rep.app = app;
    rep.version = version;
    rep.manifest = mi;
    Ok(rep)
}
