//! Crash injection: the state file must always read back as the old or the new state, never a
//! torn mix, whatever step a crash interrupts.

use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use andysu_core::fsx::{Fs, StdFs};
use andysu_core::state::{self, SlotRef, State};
use andysu_core::{Error, Result};

/// Performs the first `budget` operations for real, then "crashes": the operation in progress is
/// torn (a write lands only partially) and nothing after it happens.
struct FaultFs {
    budget: Cell<usize>,
    tear_at: usize,
}

impl FaultFs {
    fn step(&self) -> bool {
        let b = self.budget.get();
        if b == 0 {
            return false;
        }
        self.budget.set(b - 1);
        true
    }
}

impl Fs for FaultFs {
    fn write_new(&self, path: &Path, data: &[u8]) -> Result<()> {
        if self.step() {
            return StdFs.write_new(path, data);
        }
        let n = self.tear_at.min(data.len());
        let _ = fs::write(path, &data[..n]);
        Err(Error::Io("crash".into()))
    }
    fn sync_file(&self, path: &Path) -> Result<()> {
        if self.step() {
            StdFs.sync_file(path)
        } else {
            Err(Error::Io("crash".into()))
        }
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        if self.step() {
            StdFs.rename(from, to)
        } else {
            Err(Error::Io("crash".into()))
        }
    }
    fn sync_dir(&self, dir: &Path) -> Result<()> {
        if self.step() {
            StdFs.sync_dir(dir)
        } else {
            Err(Error::Io("crash".into()))
        }
    }
}

fn states() -> (State, State) {
    let mut old = State::new("11111111-2222-4333-8444-555555555555".into());
    old.stage(SlotRef { id: "1.0".into(), sequence: 1, hot_only: false });
    old.on_launch(0, true, 2);
    old.mark_healthy();
    let mut new = old.clone();
    new.stage(SlotRef { id: "2.0".into(), sequence: 2, hot_only: false });
    new.on_launch(0, true, 2);
    (old, new)
}

#[test]
fn every_crash_point_leaves_old_or_new() {
    let (old, new) = states();
    let mut seen_old = 0;
    let mut seen_new = 0;
    for budget in 0..=4 {
        for tear_at in [0usize, 1, 17, 100, usize::MAX] {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("state.json");
            state::save(&p, &old).unwrap();
            let f = FaultFs { budget: Cell::new(budget), tear_at };
            let r = state::save_with(&f, &p, &new);
            let got = state::load(&p).unwrap().expect("state.json must exist");
            assert!(got == old || got == new, "budget {budget} tear {tear_at}: torn state");
            if got == old {
                seen_old += 1
            } else {
                seen_new += 1
            }
            if budget == 4 {
                assert!(r.is_ok());
                assert_eq!(got, new);
            }
            // A later successful write recovers regardless of leftovers (e.g. a torn .tmp).
            state::save(&p, &new).unwrap();
            assert_eq!(state::load(&p).unwrap().unwrap(), new);
        }
    }
    assert!(seen_old > 0 && seen_new > 0);
}

#[test]
fn torn_staging_never_becomes_a_slot() {
    // A crash while building a slot leaves only `<id>.staging/`; launch ignores it and GC
    // removes it.
    let d = tempfile::tempdir().unwrap();
    let store = andysu_core::store::Store::open(d.path()).unwrap();
    let staging = store.staging_dir("2.0");
    fs::create_dir_all(staging.join("lib")).unwrap();
    fs::write(staging.join("lib/half.so"), b"\x7fELF partial").unwrap();
    let mut st = store.load_state([1; 16]).unwrap();
    st.stage(SlotRef { id: "2.0".into(), sequence: 2, hot_only: false });
    // (pending was recorded only after rename in real code; simulate a pending pointing at a
    // slot that never got renamed into place)
    store.save_state(&st).unwrap();
    let dcs = store.launch([1; 16], 0, true, 2).unwrap();
    assert_eq!(dcs.slot, None, "a slot that never finished staging is not loaded");
    let st = store.load_state([1; 16]).unwrap();
    let rep = store.gc(&st, false, &[]).unwrap();
    assert_eq!(rep.removed_slots, vec!["2.0.staging".to_string()]);
}

const CHILD_ENV: &str = "ANDYSU_CRASH_CHILD_DIR";

/// Child body: rewrite the state as fast as possible until killed.
#[test]
#[ignore]
fn crash_child_writer() {
    let Ok(dir) = std::env::var(CHILD_ENV) else { return };
    let p = Path::new(&dir).join("state.json");
    let mut st = State::new("11111111-2222-4333-8444-555555555555".into());
    // A big state makes each write span many pages.
    st.bad = (0..2000).map(|i| format!("bad-release-{i:05}")).collect();
    let mut i = 0u32;
    loop {
        st.live_swaps = i;
        st.last_event = Some(format!("write {i}"));
        state::save(&p, &st).unwrap();
        i += 1;
    }
}

#[test]
fn sigkill_mid_write() {
    let exe = std::env::current_exe().unwrap();
    let mut checked = 0;
    let mut writes = 0u64;
    for round in 0..25 {
        let d = tempfile::tempdir().unwrap();
        let mut child = Command::new(&exe)
            .args(["--ignored", "--exact", "crash_child_writer", "--nocapture"])
            .env(CHILD_ENV, d.path())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(40 + (round * 13 % 70) as u64));
        child.kill().unwrap(); // SIGKILL on unix
        child.wait().unwrap();
        let p = d.path().join("state.json");
        if !p.exists() {
            continue; // killed before the first rename
        }
        let st = state::load(&p).unwrap_or_else(|e| panic!("round {round}: {e}")).unwrap();
        assert_eq!(st.last_event, Some(format!("write {}", st.live_swaps)), "round {round}: torn");
        assert_eq!(st.bad.len(), 2000);
        checked += 1;
        writes += st.live_swaps as u64;
    }
    assert!(checked >= 20, "only {checked} rounds got far enough to write");
    eprintln!("sigkill_mid_write: {checked} rounds, {writes} completed writes before kills");
}
