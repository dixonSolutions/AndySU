//! Release selection: requirements, rollout buckets, ABI choice.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{Component, Manifest, Release};

/// What the running host and device offer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Host API level of the frozen host.
    pub host_api: u64,
    /// Version code of the installed shell (APK), for tier C.
    #[serde(default)]
    pub shell_version: u64,
    /// OS API level (Android SDK_INT; 0 on desktop).
    #[serde(default)]
    pub api_level: u64,
    /// Supported ABIs in preference order (Build.SUPPORTED_ABIS, or e.g. ["x86_64"] on Linux).
    #[serde(default)]
    pub abis: Vec<String>,
}

pub fn rollout_bucket(install_id: &str, salt: &str) -> u64 {
    let mut h = Sha256::new();
    h.update(install_id.as_bytes());
    h.update(b":");
    h.update(salt.as_bytes());
    let d = h.finalize();
    u64::from_be_bytes(d[..8].try_into().unwrap()) % 100
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection<'a> {
    /// Nothing newer applies.
    None,
    /// Install this release, using this ABI for native components.
    Release { release: &'a Release, abi: Option<String> },
    /// The newest release needs a newer host; it ships a shell (tier C).
    ShellRequired { release: &'a Release },
}

impl Device {
    /// The ABI to use for a release: the device's most preferred ABI that the release supports.
    pub fn abi_for(&self, r: &Release) -> Option<Option<String>> {
        let natives: Vec<&str> = r.components.iter().filter_map(|c| c.abi.as_deref()).collect();
        let supported = |a: &str| r.requires.abis.is_empty() || r.requires.abis.iter().any(|x| x == a);
        if natives.is_empty() && r.requires.abis.is_empty() {
            return Some(None);
        }
        for a in &self.abis {
            if supported(a) && (natives.is_empty() || natives.contains(&a.as_str())) {
                return Some(Some(a.clone()));
            }
        }
        None
    }

    fn meets_os(&self, r: &Release) -> bool {
        r.requires.api_min.map_or(true, |m| self.api_level >= m) && self.abi_for(r).is_some()
    }

    fn meets_host(&self, r: &Release) -> bool {
        r.requires.host_api_min.map_or(true, |m| self.host_api >= m)
            && r.requires.shell_min.map_or(true, |m| self.shell_version >= m)
    }
}

/// SPEC §5 release selection.
pub fn select<'a>(
    m: &'a Manifest,
    dev: &Device,
    installed_sequence: Option<u64>,
    bad: &[String],
    install_id: &str,
) -> Selection<'a> {
    let floor = installed_sequence.unwrap_or(0);
    let mut cands: Vec<&Release> = m
        .releases
        .iter()
        .filter(|r| installed_sequence.map_or(true, |_| r.sequence > floor))
        .filter(|r| !bad.contains(&r.id))
        .filter(|r| dev.meets_os(r))
        .filter(|r| r.includes(install_id))
        .collect();
    cands.sort_by_key(|r| std::cmp::Reverse(r.sequence));
    let Some(best) = cands.first() else { return Selection::None };
    if let Some(ok) = cands.iter().find(|r| dev.meets_host(r)) {
        if ok.sequence == best.sequence || best.shell.is_none() {
            return Selection::Release { release: ok, abi: dev.abi_for(ok).unwrap() };
        }
    }
    if best.shell.is_some() && !dev.meets_host(best) {
        return Selection::ShellRequired { release: best };
    }
    Selection::None
}

/// Components of `r` that apply to a device using `abi`.
pub fn components_for<'a>(r: &'a Release, abi: Option<&str>) -> Vec<&'a Component> {
    r.components.iter().filter(|c| c.abi.is_none() || c.abi.as_deref() == abi).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::*;

    fn comp(path: &str, abi: Option<&str>) -> Component {
        Component {
            name: "c".into(),
            kind: Kind::Content,
            apply: Apply::Restart,
            abi: abi.map(Into::into),
            path: path.into(),
            archive: None,
            unpacked_size: None,
            sha256: "0".repeat(64),
            size: 1,
            urls: vec![],
            deltas: vec![],
        }
    }

    fn rel(id: &str, seq: u64) -> Release {
        Release {
            id: id.into(),
            sequence: seq,
            requires: Requires::default(),
            rollout: None,
            mandatory: false,
            notes_url: None,
            components: vec![comp(DESCRIPTOR_PATH, None)],
            shell: None,
        }
    }

    fn man(releases: Vec<Release>) -> Manifest {
        Manifest {
            schema: 1,
            app: "a.b".into(),
            channel: "stable".into(),
            issued: "2026-01-01T00:00:00Z".into(),
            expires: "2099-01-01T00:00:00Z".into(),
            sequence: 1,
            releases,
        }
    }

    fn dev() -> Device {
        Device { host_api: 1, shell_version: 1, api_level: 30, abis: vec!["x86_64".into(), "x86".into()] }
    }

    #[test]
    fn picks_highest_eligible() {
        let mut r3 = rel("3", 3);
        r3.requires.api_min = Some(99);
        let m = man(vec![rel("1", 1), rel("2", 2), r3]);
        match select(&m, &dev(), Some(1), &[], "x") {
            Selection::Release { release, .. } => assert_eq!(release.id, "2"),
            s => panic!("{s:?}"),
        }
        assert_eq!(select(&m, &dev(), Some(2), &[], "x"), Selection::None);
        assert_eq!(select(&m, &dev(), Some(1), &["2".into()], "x"), Selection::None);
    }

    #[test]
    fn abi_choice() {
        let mut r = rel("1", 1);
        r.components.push(comp("lib/a.so", Some("arm64-v8a")));
        r.components.push(comp("lib/b.so", Some("x86")));
        let m = man(vec![r]);
        match select(&m, &dev(), None, &[], "x") {
            Selection::Release { abi, release } => {
                assert_eq!(abi.as_deref(), Some("x86"));
                assert_eq!(components_for(release, abi.as_deref()).len(), 2);
            }
            s => panic!("{s:?}"),
        }
        let arm = Device { abis: vec!["armeabi-v7a".into()], ..dev() };
        assert_eq!(select(&m, &arm, None, &[], "x"), Selection::None);
    }

    #[test]
    fn shell_required() {
        let mut r2 = rel("2", 2);
        r2.requires.host_api_min = Some(5);
        r2.shell = Some(Shell { version_code: 9, sha256: "1".repeat(64), size: 1, urls: vec![] });
        let m = man(vec![rel("1", 1), r2]);
        assert!(matches!(select(&m, &dev(), None, &[], "x"), Selection::ShellRequired { .. }));
        // Without a shell entry the newest installable one is used.
        let mut m2 = m.clone();
        m2.releases[1].shell = None;
        match select(&m2, &dev(), None, &[], "x") {
            Selection::Release { release, .. } => assert_eq!(release.id, "1"),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn rollout_is_uniformish_and_stable() {
        let mut counts = [0u32; 100];
        for i in 0..20000 {
            counts[rollout_bucket(&format!("id-{i}"), "salt") as usize] += 1;
        }
        assert!(counts.iter().all(|&c| (120..=280).contains(&c)), "{counts:?}");
        assert_eq!(rollout_bucket("abc", "s"), rollout_bucket("abc", "s"));
        let mut r = rel("1", 1);
        r.rollout = Some(Rollout { percent: 0, salt: "s".into() });
        assert!(!r.includes("anything"));
        r.rollout = Some(Rollout { percent: 100, salt: "s".into() });
        assert!(r.includes("anything"));
    }
}
