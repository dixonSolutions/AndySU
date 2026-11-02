//! `andysu pack`: turn a payload directory into a release (content-addressed blobs, deltas
//! against earlier releases, and a release JSON) inside a site directory.
//!
//! Payload directory layout:
//!
//! ```text
//! andysu-payload.json        descriptor (spec/schema/payload.schema.json)
//! dex/*.dex                  JVM code (any *.dex is kind=dex)
//! lib/<abi>/*.so             native libraries (renamed lib<name>.<hash8>.so here)
//! anything else              content
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use andysu_core::hash::sha256_hex;
use andysu_core::manifest::{
    Apply, Archive, Component, Delta, Kind, Manifest, PatchFormat, Release, Requires, DESCRIPTOR_PATH,
};
use andysu_core::payload::{Descriptor, NativeLib};
use serde::{Deserialize, Serialize};

use crate::{elf, read, write, R};

/// What the frozen host declares (written when the host APK is built; `andysu-host.json`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HostSpec {
    pub app: String,
    pub host_api: u32,
    #[serde(default)]
    pub shell_version: u64,
    #[serde(default)]
    pub min_sdk: u64,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub activities: u32,
    #[serde(default)]
    pub services: u32,
    #[serde(default)]
    pub receivers: u32,
    #[serde(default)]
    pub providers: u32,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub payload: PathBuf,
    pub site: PathBuf,
    pub channel: String,
    pub id: Option<String>,
    pub host: Option<HostSpec>,
    pub hot: Vec<String>,
    pub archive: Vec<String>,
    pub deltas: usize,
    pub delta_format: PatchFormat,
    pub base_url: Option<String>,
    pub require_16k: bool,
    pub allow_shell: bool,
    /// Also write the release as a ready slot directory (for the payload built into a host).
    pub emit_slot: Option<PathBuf>,
    /// Keep library file names and SONAMEs (transparent mode: one generation per process, and
    /// engines that dlopen("libmain.so") by name keep working).
    pub no_rename: bool,
}

#[derive(Serialize, Debug, Default)]
pub struct Report {
    pub release: String,
    pub sequence: u64,
    pub components: usize,
    pub new_blobs: Vec<String>,
    pub deltas: Vec<String>,
    pub renamed: BTreeMap<String, String>,
    pub warnings: Vec<String>,
    pub release_json: String,
}

pub fn walk(root: &Path) -> R<Vec<String>> {
    fn go(root: &Path, dir: &Path, out: &mut Vec<String>) -> R<()> {
        let mut ents: Vec<_> = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let p = e.path();
            let ft = e.file_type().map_err(|e| e.to_string())?;
            if ft.is_symlink() {
                return Err(format!("{}: symlinks are not allowed in a payload", p.display()));
            }
            if ft.is_dir() {
                go(root, &p, out)?;
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.push(rel);
            }
        }
        Ok(())
    }
    let mut v = vec![];
    go(root, root, &mut v)?;
    Ok(v)
}

/// Stable component name for a logical path (no hashes), used to match deltas across releases.
pub fn component_name(logical: &str) -> String {
    let mut n: String =
        logical.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '.' }).collect();
    while n.starts_with(['.', '_', '-']) {
        n.remove(0);
    }
    if n.len() > 128 {
        let h = &sha256_hex(logical.as_bytes())[..8];
        n = format!("{}.{h}", &n[..119]);
    }
    if n.is_empty() {
        n = format!("c{}", &sha256_hex(logical.as_bytes())[..8]);
    }
    n
}

fn hash8_name(file: &str, data: &[u8]) -> String {
    let stem = file.strip_suffix(".so").unwrap_or(file);
    format!("{stem}.{}.so", &sha256_hex(data)[..8])
}

fn already_hashed(file: &str) -> bool {
    let Some(stem) = file.strip_suffix(".so") else { return false };
    stem.rsplit_once('.').is_some_and(|(_, h)| h.len() == 8 && h.bytes().all(|c| c.is_ascii_hexdigit()))
}

/// Dependency-first order of libraries (by file name) within one ABI.
fn load_order(libs: &BTreeMap<String, Vec<String>>) -> R<Vec<String>> {
    let mut out = vec![];
    let mut state: HashMap<&str, u8> = HashMap::new();
    fn visit<'a>(
        n: &'a str,
        libs: &'a BTreeMap<String, Vec<String>>,
        st: &mut HashMap<&'a str, u8>,
        out: &mut Vec<String>,
    ) -> R<()> {
        match st.get(n) {
            Some(2) => return Ok(()),
            Some(1) => return Err(format!("dependency cycle through {n}")),
            _ => {}
        }
        st.insert(n, 1);
        for d in &libs[n] {
            if libs.contains_key(d) {
                visit(d, libs, st, out)?;
            }
        }
        st.insert(n, 2);
        out.push(n.to_string());
        Ok(())
    }
    for n in libs.keys() {
        visit(n, libs, &mut state, &mut out)?;
    }
    Ok(out)
}

pub fn load_manifest(site: &Path, channel: &str) -> R<Option<Manifest>> {
    let p = site.join(channel).join("update.json");
    if !p.exists() {
        return Ok(None);
    }
    Manifest::parse(&read(&p)?).map(Some).map_err(|e| format!("{}: {e}", p.display()))
}

/// All releases known to the site (every channel manifest plus releases/*.json).
pub fn known_releases(site: &Path) -> R<Vec<Release>> {
    let mut v = vec![];
    if let Ok(rd) = fs::read_dir(site) {
        for e in rd.flatten() {
            if e.path().join("update.json").is_file() {
                if let Some(m) = load_manifest(site, &e.file_name().to_string_lossy())? {
                    v.extend(m.releases);
                }
            }
        }
    }
    if let Ok(rd) = fs::read_dir(site.join("releases")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Ok(r) = serde_json::from_slice::<Release>(&read(&p)?) {
                    v.push(r);
                }
            }
        }
    }
    Ok(v)
}

fn blob_bytes(site: &Path, c_sha: &str, urls: &[String]) -> Option<Vec<u8>> {
    let p = site.join("blobs/sha256").join(c_sha);
    if let Ok(b) = fs::read(&p) {
        return Some(b);
    }
    for u in urls.iter().filter(|u| u.starts_with("http")) {
        if let Ok(andysu_core::transport::Got::Body { bytes, .. }) = andysu_core::transport::get(u, None) {
            if sha256_hex(&bytes) == c_sha {
                return Some(bytes);
            }
        }
    }
    None
}

/// Check what the payload needs against what the frozen host declares.
pub fn check_host(d: &Descriptor, h: &HostSpec, allow_shell: bool) -> R<Vec<String>> {
    let mut errs = vec![];
    if d.app != h.app {
        errs.push(format!("payload app {} != host app {}", d.app, h.app));
    }
    if d.host_api_min > h.host_api && !allow_shell {
        errs.push(format!(
            "payload needs host API {} but the frozen host has {}: this would need an APK update",
            d.host_api_min, h.host_api
        ));
    }
    let short = |p: &str| p.rsplit('.').next().unwrap_or(p).to_string();
    let declared: HashSet<String> = h.permissions.iter().map(|p| short(p)).collect();
    for p in &d.needs.permissions {
        if !declared.contains(&short(p)) {
            errs.push(format!(
                "payload needs permission {p}, which the frozen host does not declare: this would need an APK update"
            ));
        }
    }
    for (what, need, have) in [
        ("activities", d.needs.activities, h.activities),
        ("services", d.needs.services, h.services),
        ("receivers", d.needs.receivers, h.receivers),
        ("providers", d.needs.providers, h.providers),
    ] {
        if need > have {
            errs.push(format!(
                "payload needs {need} stub {what}, the frozen host declares {have}: this would need an APK update"
            ));
        }
    }
    for f in &d.needs.features {
        if !h.features.contains(f) {
            errs.push(format!(
                "payload needs feature {f}, which the frozen host does not declare: this would need an APK update"
            ));
        }
    }
    if errs.is_empty() {
        Ok(vec![])
    } else {
        Err(errs.join("\n"))
    }
}

fn zip_dir(dir: &Path) -> R<Vec<u8>> {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        // Fixed timestamps so identical content gives identical blobs.
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        for rel in walk(dir)? {
            w.start_file(rel.as_str(), opts).map_err(|e| e.to_string())?;
            w.write_all(&read(&dir.join(&rel))?).map_err(|e| e.to_string())?;
        }
        w.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf.into_inner())
}

/// Above this many loose files, `pack` bundles content directories into zip components.
pub const AUTO_ARCHIVE_LIMIT: usize = 400;

pub fn pack(o: &Options) -> R<Report> {
    let mut rep = Report::default();
    let dpath = o.payload.join(DESCRIPTOR_PATH);
    let mut desc = Descriptor::parse(&read(&dpath)?).map_err(|e| format!("{}: {e}", dpath.display()))?;
    if let Some(h) = &o.host {
        check_host(&desc, h, o.allow_shell)?;
    }
    let id = o.id.clone().unwrap_or_else(|| desc.version.clone());
    if !andysu_core::manifest::is_ident(&id, b"._+-", 64) {
        return Err(format!("bad release id {id:?}"));
    }

    // logical path -> (bytes, kind, abi)
    let mut files: BTreeMap<String, (Vec<u8>, Kind, Option<String>)> = BTreeMap::new();
    let mut archives: Vec<String> = o.archive.iter().map(|a| a.trim_matches('/').to_string()).collect();
    // Keep releases small in components (the spec allows 4096; a GitHub release holds 1000 assets):
    // bundle the largest content directories
    // (by file count) into zip components until at most AUTO_ARCHIVE_LIMIT files remain loose.
    let all = walk(&o.payload)?;
    let loose = |archives: &Vec<String>| {
        all.iter().filter(|r| !archives.iter().any(|a| r.starts_with(&format!("{a}/")))).count()
    };
    if loose(&archives) > AUTO_ARCHIVE_LIMIT {
        let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
        for r in &all {
            let parts: Vec<&str> = r.split('/').collect();
            // Candidate directories: anything below the top level except code (dex/, lib/).
            for depth in 1..parts.len() {
                let d = parts[..depth].join("/");
                if d == "dex"
                    || d == "lib"
                    || d.starts_with("lib/")
                    || depth == 1 && parts[0] != "assets" && parts[0] != "res"
                {
                    continue;
                }
                *dirs.entry(d).or_default() += 1;
            }
        }
        // Prefer deeper directories (smaller archives update better), largest first.
        let mut cands: Vec<(String, usize)> = dirs.into_iter().filter(|(d, n)| *n >= 16 && d.contains('/')).collect();
        cands.sort_by_key(|(d, n)| (std::cmp::Reverse(d.matches('/').count().min(2)), std::cmp::Reverse(*n)));
        for (d, _) in cands {
            if loose(&archives) <= AUTO_ARCHIVE_LIMIT {
                break;
            }
            if archives.iter().any(|a| d.starts_with(&format!("{a}/")) || a.starts_with(&format!("{d}/")) || *a == d) {
                continue;
            }
            archives.push(d);
        }
        if loose(&archives) > AUTO_ARCHIVE_LIMIT {
            let top: Vec<String> = all
                .iter()
                .filter_map(|r| r.split_once('/').map(|x| x.0.to_string()))
                .filter(|t| t == "assets")
                .collect();
            if !top.is_empty() && !archives.contains(&"assets".to_string()) {
                archives.retain(|a| !a.starts_with("assets/"));
                archives.push("assets".into());
            }
        }
        rep.warnings.push(format!("{} files: bundled {} directories into zip components", all.len(), archives.len()));
    }
    for rel in walk(&o.payload)? {
        if rel == DESCRIPTOR_PATH || archives.iter().any(|a| rel.starts_with(&format!("{a}/"))) {
            continue;
        }
        andysu_core::manifest::check_rel_path(&rel).map_err(|e| e.to_string())?;
        let data = read(&o.payload.join(&rel))?;
        let parts: Vec<&str> = rel.split('/').collect();
        let (kind, abi) = if parts.len() == 3 && parts[0] == "lib" && rel.ends_with(".so") {
            (Kind::Native, Some(parts[1].to_string()))
        } else if rel.ends_with(".dex") {
            (Kind::Dex, None)
        } else {
            (Kind::Content, None)
        };
        files.insert(rel, (data, kind, abi));
    }

    // Native libraries: unique names, DT_NEEDED rewrite, alignment check, load order.
    let mut by_abi: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (rel, (_, k, abi)) in &files {
        if *k == Kind::Native {
            by_abi.entry(abi.clone().unwrap()).or_default().push(rel.clone());
        }
    }
    let mut renamed_files: BTreeMap<String, (Vec<u8>, Kind, Option<String>, String)> = BTreeMap::new(); // new path -> (bytes, kind, abi, logical)
    for (abi, rels) in &by_abi {
        let mut rename: HashMap<String, String> = HashMap::new();
        for rel in rels {
            let file = rel.rsplit('/').next().unwrap();
            let new =
                if o.no_rename || already_hashed(file) { file.to_string() } else { hash8_name(file, &files[rel].0) };
            rename.insert(file.to_string(), new);
        }
        let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for rel in rels {
            let file = rel.rsplit('/').next().unwrap().to_string();
            let data = &files[rel].0;
            let info = elf::info(data).map_err(|e| format!("{rel}: {e}"))?;
            if info.abi() != abi {
                return Err(format!("{rel}: ELF machine is {} but it sits in lib/{abi}/", info.abi()));
            }
            let al = info.min_load_align();
            if al < 16384 {
                let msg = format!("{rel}: LOAD segments aligned to {al} bytes, not 16 KB; Android 15+ devices with 16 KB pages cannot load it (link with -Wl,-z,max-page-size=16384)");
                if o.require_16k {
                    return Err(msg);
                }
                rep.warnings.push(msg);
            }
            let new_name = &rename[&file];
            let soname = info.soname.as_ref().map(|_| new_name.as_str());
            let out = if o.no_rename {
                data.clone()
            } else {
                elf::rewrite(data, soname, &rename).map_err(|e| format!("{rel}: {e}"))?
            };
            deps.insert(file.clone(), info.needed.clone());
            let new_rel = format!("lib/{abi}/{new_name}");
            if new_rel != *rel {
                rep.renamed.insert(rel.clone(), new_rel.clone());
            }
            renamed_files.insert(new_rel, (out, Kind::Native, Some(abi.clone()), rel.clone()));
        }
        let order = load_order(&deps)?;
        desc.native.insert(
            abi.clone(),
            order
                .iter()
                .map(|f| NativeLib {
                    name: f.strip_suffix(".so").unwrap_or(f).to_string(),
                    path: format!("lib/{abi}/{}", rename[f]),
                })
                .collect(),
        );
    }
    if let Some(entry) = &desc.entry.native_lib {
        for abi in by_abi.keys() {
            if desc.native_path(abi, entry).is_none() {
                return Err(format!("entry.native_lib {entry} is missing for {abi}"));
            }
        }
    }
    if desc.dex.is_empty() {
        let mut dex: Vec<String> = files.iter().filter(|(_, v)| v.1 == Kind::Dex).map(|(k, _)| k.clone()).collect();
        // classes.dex, classes2.dex, classes10.dex in numeric order
        dex.sort_by_key(|p| {
            let f = p.rsplit('/').next().unwrap();
            let n: u32 = f.trim_start_matches("classes").trim_end_matches(".dex").parse().unwrap_or(1);
            (p.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default(), n, p.clone())
        });
        desc.dex = dex;
    }
    desc.validate().map_err(|e| e.to_string())?;

    // Assemble the component list: (path, logical, bytes, kind, abi, archive)
    // (slot path, logical path, bytes, kind, abi, archive)
    type Entry = (String, String, Vec<u8>, Kind, Option<String>, Option<Archive>);
    let mut entries: Vec<Entry> = vec![];
    let mut dj = serde_json::to_vec_pretty(&desc).unwrap();
    dj.push(b'\n');
    entries.push((DESCRIPTOR_PATH.into(), DESCRIPTOR_PATH.into(), dj, Kind::Content, None, None));
    for (rel, (data, kind, abi)) in files {
        if kind != Kind::Native {
            entries.push((rel.clone(), rel, data, kind, abi, None));
        }
    }
    for (rel, (data, kind, abi, logical)) in renamed_files {
        entries.push((rel, logical, data, kind, abi, None));
    }
    for a in &archives {
        let dir = o.payload.join(a);
        if !dir.is_dir() {
            return Err(format!("--archive {a}: not a directory in the payload"));
        }
        entries.push((a.clone(), format!("{a}/"), zip_dir(&dir)?, Kind::Content, None, Some(Archive::Zip)));
    }

    // Previous releases for deltas and URL reuse.
    let known = known_releases(&o.site)?;
    if known.iter().any(|r| r.id == id) {
        return Err(format!("release {id} already exists in {}", o.site.display()));
    }
    let sequence = known.iter().map(|r| r.sequence).max().unwrap_or(0) + 1;
    let chan = load_manifest(&o.site, &o.channel)?;
    let mut prev: Vec<&Release> = chan.as_ref().map(|m| m.releases.iter().collect()).unwrap_or_default();
    prev.sort_by_key(|r| std::cmp::Reverse(r.sequence));
    prev.truncate(o.deltas);
    let known_urls: HashMap<String, Vec<String>> = known
        .iter()
        .flat_map(|r| r.components.iter())
        .filter(|c| !c.urls.is_empty())
        .map(|c| (c.sha256.clone(), c.urls.clone()))
        .collect();

    let blobs = o.site.join("blobs/sha256");
    let mut comps = vec![];
    for (path, logical, data, kind, abi, archive) in entries {
        let sha = sha256_hex(&data);
        let bp = blobs.join(&sha);
        if !bp.exists() && !known_urls.contains_key(&sha) {
            write(&bp, &data)?;
            rep.new_blobs.push(sha.clone());
        }
        let name = component_name(&match &abi {
            Some(a) if !logical.starts_with("lib/") => format!("{a}/{logical}"),
            _ => logical.clone(),
        });
        let apply = if kind == Kind::Content
            && path != DESCRIPTOR_PATH
            && o.hot.iter().any(|h| logical.starts_with(h.as_str()))
        {
            Apply::Hot
        } else {
            Apply::Restart
        };
        let urls = match (&o.base_url, known_urls.get(&sha)) {
            (_, Some(u)) => u.clone(),
            (Some(b), None) => vec![format!("{}/{sha}", b.trim_end_matches('/'))],
            (None, None) => vec![],
        };
        let unpacked_size = if archive.is_some() {
            Some(
                walk(&o.payload.join(&path))?
                    .iter()
                    .map(|r| fs::metadata(o.payload.join(&path).join(r)).map(|m| m.len()).unwrap_or(0))
                    .sum(),
            )
        } else {
            None
        };
        // Deltas from the same logical component in recent releases.
        let mut deltas: Vec<Delta> = vec![];
        if path != DESCRIPTOR_PATH && data.len() > 4096 {
            let mut seen = HashSet::new();
            for r in &prev {
                for oc in r
                    .components
                    .iter()
                    .filter(|c| c.name == name && c.abi == abi && c.sha256 != sha && c.archive == archive)
                {
                    if !seen.insert(oc.sha256.clone()) {
                        continue;
                    }
                    let Some(old) = blob_bytes(&o.site, &oc.sha256, &oc.urls) else {
                        rep.warnings.push(format!("no local copy of {} ({}); skipping delta", oc.sha256, name));
                        continue;
                    };
                    let p = andysu_core::patch::create(o.delta_format, &old, &data).map_err(|e| e.to_string())?;
                    if (p.len() as f64) > 0.8 * data.len() as f64 {
                        continue;
                    }
                    let ph = sha256_hex(&p);
                    let dp = o.site.join(format!("deltas/{}-{sha}", oc.sha256)).join(&ph);
                    if !dp.exists() {
                        write(&dp, &p)?;
                        rep.new_blobs.push(format!("deltas/{}-{sha}/{ph}", oc.sha256));
                    }
                    let urls = o
                        .base_url
                        .as_ref()
                        .map(|b| vec![format!("{}/{ph}", b.trim_end_matches('/'))])
                        .unwrap_or_default();
                    rep.deltas.push(format!("{name}: {} -> {} bytes (from {})", data.len(), p.len(), r.id));
                    deltas.push(Delta {
                        from_sha256: oc.sha256.clone(),
                        format: o.delta_format,
                        sha256: ph,
                        size: p.len() as u64,
                        urls,
                    });
                }
            }
        }
        comps.push(Component {
            name,
            kind,
            apply,
            abi,
            path,
            archive,
            unpacked_size,
            sha256: sha,
            size: data.len() as u64,
            urls,
            deltas,
        });
    }

    let abis: Vec<String> = by_abi.keys().cloned().collect();
    let release = Release {
        id: id.clone(),
        sequence,
        requires: Requires {
            host_api_min: Some(desc.host_api_min as u64),
            shell_min: None,
            api_min: o.host.as_ref().map(|h| h.min_sdk).filter(|&v| v > 0),
            abis,
        },
        rollout: None,
        mandatory: false,
        notes_url: None,
        components: comps,
        shell: None,
    };
    release.validate().map_err(|e| e.to_string())?;
    if let Some(dir) = &o.emit_slot {
        if dir.exists() {
            andysu_core::fsx::remove_tree(dir).map_err(|e| e.to_string())?;
        }
        for c in &release.components {
            let b = blob_bytes(&o.site, &c.sha256, &c.urls).ok_or_else(|| format!("blob {} missing", c.sha256))?;
            if c.archive.is_some() {
                let tmp = dir.join(format!(".{}.zip", c.sha256));
                write(&tmp, &b)?;
                fs::create_dir_all(dir.join(&c.path)).map_err(|e| e.to_string())?;
                andysu_core::extract::extract_zip(&tmp, &dir.join(&c.path), c.unpacked_size.unwrap_or(4 << 30))
                    .map_err(|e| e.to_string())?;
                fs::remove_file(&tmp).map_err(|e| e.to_string())?;
            } else {
                write(&dir.join(&c.path), &b)?;
            }
        }
        // A slot index, so clients can use this built-in payload as a delta base.
        let ix = andysu_core::store::SlotIndex {
            id: release.id.clone(),
            sequence: release.sequence,
            abi: None,
            components: release
                .components
                .iter()
                .map(|c| andysu_core::store::IndexEntry {
                    name: c.name.clone(),
                    path: c.path.clone(),
                    sha256: c.sha256.clone(),
                    kind: c.kind,
                    apply: c.apply,
                    archive: c.archive,
                })
                .collect(),
        };
        write(&dir.join(andysu_core::store::BUILTIN_INDEX), &serde_json::to_vec_pretty(&ix).unwrap())?;
    }
    let rp = o.site.join("releases").join(format!("{id}.json"));
    write(&rp, &serde_json::to_vec_pretty(&release).unwrap())?;
    rep.release = id;
    rep.sequence = sequence;
    rep.components = release.components.len();
    rep.release_json = rp.display().to_string();
    Ok(rep)
}
