//! `andysu init` and the shared `andysu.toml` model.
//!
//! ```toml
//! app = "com.example.app"
//! host_api = 1
//! public_key = "andysu-public.pem"
//! [remote]  type = "github"  repo = "owner/app"  channel = "stable"   # or type = "custom", base = "https://..."
//! [policy]  mode = "auto"  network = "unmetered"  apply = "live"
//! [host]    permissions = ["CAMERA"]  stubs = { activities = 4, services = 2 }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{write, R};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Remote {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default = "stable")]
    pub channel: String,
}

fn stable() -> String {
    "stable".into()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Policy {
    #[serde(default = "auto")]
    pub mode: String,
    #[serde(default = "any")]
    pub network: String,
    /// live (default) | restart | next_start
    #[serde(default = "live")]
    pub apply: String,
    /// apply = restart: now | background
    #[serde(default = "now")]
    pub restart_when: String,
    /// apply = live when a live swap is impossible: restart | next_start
    #[serde(default = "next_start")]
    pub live_fallback: String,
    #[serde(default)]
    pub charging: bool,
    #[serde(default = "yes")]
    pub battery_not_low: bool,
    #[serde(default = "six_hours")]
    pub check_interval_minutes: u64,
    #[serde(default = "watchdog")]
    pub watchdog_ms: u64,
}

fn auto() -> String {
    "auto".into()
}
fn any() -> String {
    "any".into()
}
fn live() -> String {
    "live".into()
}
fn now() -> String {
    "now".into()
}
fn next_start() -> String {
    "next_start".into()
}
fn yes() -> bool {
    true
}
fn six_hours() -> u64 {
    360
}
fn watchdog() -> u64 {
    10_000
}

impl Default for Policy {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Stubs {
    #[serde(default = "four")]
    pub activities: u32,
    #[serde(default = "two")]
    pub services: u32,
    #[serde(default = "two")]
    pub receivers: u32,
    #[serde(default = "one")]
    pub providers: u32,
}

fn four() -> u32 {
    4
}
fn two() -> u32 {
    2
}
fn one() -> u32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Host {
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub stubs: Option<Stubs>,
    #[serde(default)]
    pub shell_version: u64,
    #[serde(default)]
    pub min_sdk: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AndySUToml {
    pub app: String,
    #[serde(default = "one")]
    pub host_api: u32,
    pub public_key: String,
    pub remote: Remote,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub host: Host,
}

impl AndySUToml {
    pub fn load(p: &Path) -> R<AndySUToml> {
        let s = fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let t: AndySUToml = toml::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))?;
        let check = |what: &str, v: &str, ok: &[&str]| -> R<()> {
            if ok.contains(&v) {
                Ok(())
            } else {
                Err(format!("{}: policy.{what} = {v:?}, expected one of {ok:?}", p.display()))
            }
        };
        check("mode", &t.policy.mode, &["auto", "download_only", "check_only", "off"])?;
        check("apply", &t.policy.apply, &["live", "restart", "next_start"])?;
        check("restart_when", &t.policy.restart_when, &["now", "background"])?;
        check("live_fallback", &t.policy.live_fallback, &["restart", "next_start"])?;
        check("network", &t.policy.network, &["any", "unmetered"])?;
        Ok(t)
    }

    /// Manifest URL the host fetches.
    pub fn manifest_url(&self) -> R<String> {
        let c = &self.remote.channel;
        match self.remote.kind.as_str() {
            "github" => {
                let repo =
                    self.remote.repo.as_deref().ok_or("[remote] type = \"github\" needs repo = \"owner/name\"")?;
                Ok(if c == "stable" {
                    format!("https://github.com/{repo}/releases/latest/download/update.json")
                } else {
                    format!("https://github.com/{repo}/releases/download/channel-{c}/update.json")
                })
            }
            "pages" => {
                let repo = self.remote.repo.as_deref().ok_or("[remote] type = \"pages\" needs repo")?;
                let (owner, name) = repo.split_once('/').ok_or("repo must be owner/name")?;
                Ok(format!("https://{}.github.io/{name}/{c}/update.json", owner.to_lowercase()))
            }
            "custom" => {
                let base =
                    self.remote.base.as_deref().ok_or("[remote] type = \"custom\" needs base = \"https://...\"")?;
                Ok(format!("{}/{c}/update.json", base.trim_end_matches('/')))
            }
            k => Err(format!("unknown remote type {k:?} (github, pages, custom)")),
        }
    }

    /// The host's `assets/andysu/config.json`.
    pub fn host_config(&self, dir: &Path) -> R<serde_json::Value> {
        let pem = fs::read_to_string(dir.join(&self.public_key)).map_err(|e| format!("{}: {e}", self.public_key))?;
        andysu_core::keys::PublicKey::parse_any(&pem).map_err(|e| e.to_string())?;
        let s = self.host.stubs.clone().unwrap_or(Stubs { activities: 4, services: 2, receivers: 2, providers: 1 });
        Ok(serde_json::json!({
            "app": self.app, "channel": self.remote.channel, "roots": [pem], "manifest_url": self.manifest_url()?,
            "mode": self.policy.mode, "apply": self.policy.apply, "restart_when": self.policy.restart_when,
            "live_fallback": self.policy.live_fallback, "network": self.policy.network,
            "charging": self.policy.charging, "battery_not_low": self.policy.battery_not_low,
            "check_interval_minutes": self.policy.check_interval_minutes, "watchdog_ms": self.policy.watchdog_ms,
            "host_api": self.host_api, "shell_version": self.host.shell_version,
            "stub_activities": s.activities, "stub_services": s.services,
        }))
    }

    /// `andysu-host.json`: what the frozen host declares (input to `andysu pack --host`).
    pub fn host_spec(&self) -> crate::pack::HostSpec {
        let s = self.host.stubs.clone().unwrap_or(Stubs { activities: 4, services: 2, receivers: 2, providers: 1 });
        let mut perms: Vec<String> = ["INTERNET", "ACCESS_NETWORK_STATE", "RECEIVE_BOOT_COMPLETED"]
            .iter()
            .map(|p| format!("android.permission.{p}"))
            .collect();
        // Declared by the host AAR for AndroidX core in payloads (see its AndroidManifest.xml).
        perms.push(format!("{}.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION", self.app));
        for p in &self.host.permissions {
            let full = if p.contains('.') { p.clone() } else { format!("android.permission.{p}") };
            if !perms.contains(&full) {
                perms.push(full);
            }
        }
        crate::pack::HostSpec {
            app: self.app.clone(),
            host_api: self.host_api,
            shell_version: self.host.shell_version,
            min_sdk: self.host.min_sdk,
            permissions: perms,
            activities: s.activities,
            services: s.services,
            receivers: s.receivers,
            providers: s.providers,
            features: self.host.features.clone(),
        }
    }
}

/// Project kinds `init` recognises, with the adoption snippet for each.
pub fn detect(dir: &Path) -> &'static str {
    let has = |p: &str| dir.join(p).exists();
    let read = |p: &str| fs::read_to_string(dir.join(p)).unwrap_or_default();
    if has("pubspec.yaml") && read("pubspec.yaml").contains("flutter") {
        "flutter"
    } else if has("project.godot") {
        "godot"
    } else if has("ProjectSettings/ProjectVersion.txt") {
        "unity"
    } else if has("capacitor.config.ts") || has("capacitor.config.json") {
        "capacitor"
    } else if has("package.json") && read("package.json").contains("\"expo\"") {
        "expo"
    } else if has("package.json") && read("package.json").contains("react-native") {
        "react-native"
    } else if has("config.xml") && read("config.xml").contains("cordova") {
        "cordova"
    } else if has("settings.gradle.kts") || has("settings.gradle") {
        "gradle"
    } else if has("CMakeLists.txt") {
        "cmake"
    } else {
        "generic"
    }
}

fn next_steps(kind: &str) -> &'static str {
    match kind {
        "gradle" => "Add the plugin: plugins { id(\"io.github.dixonsolutions.andysu\") } (see docs/REGISTRIES.md),\nimplement com.andysu.api.Payload, and set andysu { role = \"payload\"; entryClass = \"...\" }.",
        "flutter" => "Add the andysu Flutter package (bindings/flutter) and build with `andysu split build/app/outputs/flutter-apk/app-release.apk`.",
        "react-native" | "expo" => "Add @andysu/react-native (bindings/react-native); Expo: add the config plugin to app.json.",
        "capacitor" | "cordova" => "Add @andysu/capacitor (bindings/capacitor); web assets update hot.",
        "unity" => "Add the com.andysu.unity package (bindings/unity) through the scoped registry.",
        "godot" => "Copy bindings/godot/addons/andysu into the project.",
        "cmake" => "Link AndySU::payload (bindings/c) and export ANDYSU_PAYLOAD(start, suspend).",
        _ => "Build your APK, then: andysu split app.apk --out build/andysu --entry-class <Payload class>.",
    }
}

const WORKFLOW: &str = r#"# Generated by `andysu init`. Publishes a signed payload on every push to main.
name: andysu-publish
on:
  push:
    branches: [main]
permissions:
  contents: write
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      # Build your app's payload here (for Gradle: ./gradlew andysuPayloadRelease).
      - uses: dixonSolutions/AndySU/actions/publish@main
        with:
          payload: build/andysu/payload
          config: andysu.toml
          signing-key: ${{ secrets.ANDYSU_SIGNING_KEY }}
"#;

pub fn init(dir: &Path, app: Option<String>, repo: Option<String>, keys: bool) -> R<String> {
    let kind = detect(dir);
    let toml_path = dir.join("andysu.toml");
    if toml_path.exists() {
        return Err(format!("{} already exists", toml_path.display()));
    }
    let app = app.ok_or("--app is required (the application id, e.g. com.example.app)")?;
    if !andysu_core::manifest::is_app_id(&app) {
        return Err(format!("{app:?} is not a valid application id"));
    }
    let mut out = format!("Detected a {kind} project.\n");
    let keydir: PathBuf = dir.join(".andysu");
    if keys {
        fs::create_dir_all(&keydir).map_err(|e| e.to_string())?;
        let g = crate::keys::keygen(&keydir, "andysu")?;
        fs::rename(&g.public, dir.join("andysu-public.pem")).map_err(|e| e.to_string())?;
        out.push_str(&format!(
            "Generated root key {} in .andysu/andysu-root.key: store it as the ANDYSU_SIGNING_KEY secret\n\
             (gh secret set ANDYSU_SIGNING_KEY < .andysu/andysu-root.key), keep a copy offline, then delete it here.\n",
            g.id
        ));
        let gi = dir.join(".gitignore");
        let mut s = fs::read_to_string(&gi).unwrap_or_default();
        if !s.contains(".andysu/") {
            s.push_str("\n# AndySU private keys never go into git\n.andysu/\n*.key\n");
            write(&gi, s.as_bytes())?;
        }
    }
    let (kind_r, repo_line) = match &repo {
        Some(r) => ("github", format!("repo = \"{r}\"")),
        None => ("custom", "base = \"https://example.com/updates\"".to_string()),
    };
    let t = format!(
        "app = \"{app}\"\nhost_api = 1\npublic_key = \"andysu-public.pem\"\n\n[remote]\ntype = \"{kind_r}\"\n{repo_line}\nchannel = \"stable\"\n\n\
         [policy]\nmode = \"auto\"          # auto | download_only | check_only | off\nnetwork = \"any\"        # any | unmetered\napply = \"live\"          # live | restart | next_start\nrestart_when = \"now\"    # for apply = restart: now | background\n\n\
         [host]\n# Everything the app may ever need, declared once in the frozen host:\npermissions = []\nstubs = {{ activities = 4, services = 2 }}\n"
    );
    write(&toml_path, t.as_bytes())?;
    AndySUToml::load(&toml_path)?;
    write(&dir.join(".github/workflows/andysu.yml"), WORKFLOW.as_bytes())?;
    out.push_str("Wrote andysu.toml and .github/workflows/andysu.yml.\n\nNext: ");
    out.push_str(next_steps(kind));
    Ok(out)
}
