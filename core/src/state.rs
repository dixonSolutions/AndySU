//! Client state (`state.json`) and the slot state machine (SPEC §7).
//!
//! Transitions are pure functions on [`State`]; persistence is a separate atomic write, so the
//! caller always persists *before* acting on a decision (e.g. before loading a slot).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fsx::{atomic_write_with, Fs, StdFs};
use crate::{Error, Result};

pub const DEFAULT_MAX_BOOT_ATTEMPTS: u32 = 2;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SlotRef {
    pub id: String,
    pub sequence: u64,
    /// True when every component that changed from the current slot is `apply: hot` content.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hot_only: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Trial {
    pub boot_attempts: u32,
    #[serde(default)]
    pub started: i64,
    /// Entered by a live swap in a running process rather than by a launch.
    #[serde(default)]
    pub live: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct LastManifest {
    pub sequence: u64,
    pub issued: i64,
    pub sha256: String,
    #[serde(default)]
    pub etag: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub v: u32,
    pub install_id: String,
    #[serde(default)]
    pub current: Option<SlotRef>,
    #[serde(default)]
    pub previous: Option<SlotRef>,
    #[serde(default)]
    pub pending: Option<SlotRef>,
    #[serde(default)]
    pub trial: Option<Trial>,
    #[serde(default)]
    pub bad: Vec<String>,
    #[serde(default)]
    pub last_manifest: Option<LastManifest>,
    #[serde(default)]
    pub live_swaps: u32,
    #[serde(default)]
    pub last_event: Option<String>,
}

/// What happened during a transition, for logs and UI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Promoted { id: String },
    TrialBoot { id: String, attempt: u32 },
    RolledBack { bad: String, to: Option<String>, reason: String },
    Committed { id: String },
    Staged { id: String },
}

/// Where the bootloader should go after `on_launch`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LaunchDecision {
    /// Slot to load, or `None` for the payload built into the host (factory).
    pub slot: Option<SlotRef>,
    pub in_trial: bool,
    pub events: Vec<Event>,
}

/// UUID v4 text from 16 random bytes supplied by the caller.
pub fn uuid_from_bytes(mut b: [u8; 16]) -> String {
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

impl State {
    pub fn new(install_id: String) -> State {
        State {
            v: 1,
            install_id,
            current: None,
            previous: None,
            pending: None,
            trial: None,
            bad: vec![],
            last_manifest: None,
            live_swaps: 0,
            last_event: None,
        }
    }

    pub fn installed_sequence(&self) -> Option<u64> {
        self.current.as_ref().map(|s| s.sequence)
    }

    fn note(&mut self, e: &Event) {
        self.last_event = Some(serde_json::to_string(e).unwrap());
    }

    /// Record a newly staged slot as pending.
    pub fn stage(&mut self, slot: SlotRef) -> Event {
        let e = Event::Staged { id: slot.id.clone() };
        self.pending = Some(slot);
        self.note(&e);
        e
    }

    /// Make the pending slot current and enter TRIAL.
    fn promote(&mut self, now: i64, live: bool) -> Option<Event> {
        let p = self.pending.take()?;
        if self.bad.contains(&p.id) {
            return None;
        }
        if let Some(cur) = self.current.take() {
            self.previous = Some(cur);
        }
        let e = Event::Promoted { id: p.id.clone() };
        self.current = Some(p);
        self.trial = Some(Trial { boot_attempts: 0, started: now, live });
        self.note(&e);
        Some(e)
    }

    /// Roll back the current (trial) slot: mark it bad, restore previous.
    pub fn rollback(&mut self, reason: &str) -> Option<Event> {
        let cur = self.current.take()?;
        if !self.bad.contains(&cur.id) {
            self.bad.push(cur.id.clone());
        }
        self.current = self.previous.take();
        self.trial = None;
        let e =
            Event::RolledBack { bad: cur.id, to: self.current.as_ref().map(|s| s.id.clone()), reason: reason.into() };
        self.note(&e);
        Some(e)
    }

    /// Called by the bootloader on every cold start, *before* loading code. Persist the result
    /// before acting on it.
    pub fn on_launch(&mut self, now: i64, apply_pending: bool, max_boot_attempts: u32) -> LaunchDecision {
        let mut events = vec![];
        if apply_pending && self.pending.is_some() {
            // A pending slot only replaces a committed one; never stack trials.
            if self.trial.is_none() {
                events.extend(self.promote(now, false));
            }
        }
        if let Some(t) = &mut self.trial {
            t.boot_attempts += 1;
            let attempt = t.boot_attempts;
            if attempt > max_boot_attempts {
                events.extend(self.rollback(&format!("crash loop: {} launches without markHealthy", attempt - 1)));
            } else if let Some(c) = &self.current {
                let e = Event::TrialBoot { id: c.id.clone(), attempt };
                self.note(&e);
                events.push(e);
            }
        }
        LaunchDecision { slot: self.current.clone(), in_trial: self.trial.is_some(), events }
    }

    /// A running process swaps generations live: pending becomes current in TRIAL. The attempt
    /// counter starts at 1 so that a process death during the trial counts as a failed boot.
    pub fn begin_live(&mut self, now: i64) -> Result<Event> {
        if self.trial.is_some() {
            return Err(Error::State("already in trial".into()));
        }
        let e = self.promote(now, true).ok_or_else(|| Error::State("nothing pending".into()))?;
        if let Some(t) = &mut self.trial {
            t.boot_attempts = 1;
        }
        self.live_swaps += 1;
        Ok(e)
    }

    /// The payload called markHealthy(): COMMIT.
    pub fn mark_healthy(&mut self) -> Option<Event> {
        self.trial.take()?;
        let e = Event::Committed { id: self.current.as_ref()?.id.clone() };
        self.note(&e);
        Some(e)
    }

    /// Init exception, uncaught crash, native signal or watchdog timeout. Only acts in TRIAL;
    /// a committed slot is never rolled back automatically.
    pub fn report_failure(&mut self, reason: &str) -> Option<Event> {
        if self.trial.is_some() {
            self.rollback(reason)
        } else {
            None
        }
    }

    /// Reset counters that only make sense within one process (called on cold start).
    pub fn cold_start(&mut self) {
        self.live_swaps = 0;
    }

    /// Slot ids to keep on disk.
    pub fn keep(&self, low_storage: bool) -> Vec<String> {
        let mut v = vec![];
        if let Some(c) = &self.current {
            v.push(c.id.clone());
        }
        if !low_storage || self.trial.is_some() {
            if let Some(p) = &self.previous {
                v.push(p.id.clone());
            }
        }
        if let Some(p) = &self.pending {
            v.push(p.id.clone());
        }
        v
    }
}

/// Load `state.json`. A missing file yields `None`; a corrupt one is an error (it can only come
/// from disk damage, since writes are atomic).
pub fn load(path: &Path) -> Result<Option<State>> {
    match std::fs::read(path) {
        Ok(b) => {
            let s: State =
                serde_json::from_slice(&b).map_err(|e| Error::State(format!("corrupt {}: {e}", path.display())))?;
            if s.v != 1 {
                return Err(Error::State(format!("unsupported state version {}", s.v)));
            }
            Ok(Some(s))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(format!("read {}: {e}", path.display()))),
    }
}

pub fn save_with(fs: &dyn Fs, path: &Path, s: &State) -> Result<()> {
    let mut b = serde_json::to_vec_pretty(s).unwrap();
    b.push(b'\n');
    atomic_write_with(fs, path, &b)
}

pub fn save(path: &Path, s: &State) -> Result<()> {
    save_with(&StdFs, path, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(id: &str, seq: u64) -> SlotRef {
        SlotRef { id: id.into(), sequence: seq, hot_only: false }
    }

    #[test]
    fn happy_path() {
        let mut s = State::new("x".into());
        let d = s.on_launch(0, true, 2);
        assert_eq!(d.slot, None);
        s.stage(slot("a", 1));
        let d = s.on_launch(0, true, 2);
        assert_eq!(d.slot.unwrap().id, "a");
        assert!(d.in_trial);
        assert!(s.mark_healthy().is_some());
        assert!(s.trial.is_none());
        s.stage(slot("b", 2));
        let d = s.on_launch(0, true, 2);
        assert_eq!(d.slot.unwrap().id, "b");
        assert_eq!(s.previous.as_ref().unwrap().id, "a");
        s.mark_healthy();
        assert_eq!(s.keep(false), vec!["b".to_string(), "a".to_string()]);
        assert_eq!(s.keep(true), vec!["b".to_string()]);
    }

    #[test]
    fn crash_loop_rolls_back_after_n() {
        let mut s = State::new("x".into());
        s.stage(slot("a", 1));
        s.on_launch(0, true, 2);
        s.mark_healthy();
        s.stage(slot("b", 2));
        assert_eq!(s.on_launch(0, true, 2).slot.unwrap().id, "b"); // attempt 1
        assert_eq!(s.on_launch(0, true, 2).slot.unwrap().id, "b"); // attempt 2
        let d = s.on_launch(0, true, 2); // attempt 3 > 2: roll back before loading
        assert_eq!(d.slot.unwrap().id, "a");
        assert!(!d.in_trial);
        assert_eq!(s.bad, vec!["b".to_string()]);
        // A bad release is never promoted again.
        s.stage(slot("b", 2));
        assert_eq!(s.on_launch(0, true, 2).slot.unwrap().id, "a");
    }

    #[test]
    fn failure_only_in_trial() {
        let mut s = State::new("x".into());
        s.stage(slot("a", 1));
        s.on_launch(0, true, 2);
        let e = s.report_failure("init exception").unwrap();
        assert!(matches!(e, Event::RolledBack { to: None, .. }));
        assert_eq!(s.current, None);
        s.stage(slot("c", 3));
        s.on_launch(0, true, 2);
        s.mark_healthy();
        assert!(s.report_failure("crash").is_none());
        assert_eq!(s.current.as_ref().unwrap().id, "c");
    }

    #[test]
    fn live_trial_counts_process_death() {
        let mut s = State::new("x".into());
        s.stage(slot("a", 1));
        s.on_launch(0, true, 2);
        s.mark_healthy();
        s.stage(slot("b", 2));
        s.begin_live(5).unwrap();
        assert_eq!(s.trial.as_ref().unwrap().boot_attempts, 1);
        // Process killed during the live trial, then two crashing relaunches.
        assert_eq!(s.on_launch(6, true, 2).slot.unwrap().id, "b");
        assert_eq!(s.on_launch(7, true, 2).slot.unwrap().id, "a");
        assert!(s.begin_live(8).is_err());
    }

    #[test]
    fn uuid_format() {
        let u = uuid_from_bytes([0xff; 16]);
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
