//! The ELF rewriter against real toolchains: rename SONAME/DT_NEEDED, then load two
//! generations side by side with dlopen and check each uses its own dependency copy.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use andysu_cli::elf;

fn cc(args: &[&str], dir: &Path) {
    let st = Command::new("cc").args(args).current_dir(dir).status().expect("cc");
    assert!(st.success(), "cc {args:?}");
}

fn build_gen(dir: &Path, v: u32) {
    fs::write(dir.join("dep.c"), format!("int dep_value(void) {{ return {v}; }}\nint dep_counter;\nint *dep_counter_addr(void) {{ return &dep_counter; }}\n")).unwrap();
    fs::write(dir.join("game.c"), "int dep_value(void); int *dep_counter_addr(void);\nint game_value(void) { return dep_value() * 10; }\nint *game_dep_counter(void) { return dep_counter_addr(); }\n").unwrap();
    cc(&["-shared", "-fPIC", "-Wl,-soname,libdep.so", "-o", "libdep.so", "dep.c"], dir);
    cc(&["-shared", "-fPIC", "-Wl,-soname,libgame.so", "-o", "libgame.so", "game.c", "-L.", "-ldep"], dir);
}

fn rename(dir: &Path, tag: &str) -> (String, String) {
    let dep_new = format!("libdep.{tag}.so");
    let game_new = format!("libgame.{tag}.so");
    let dep = elf::rewrite(&fs::read(dir.join("libdep.so")).unwrap(), Some(&dep_new), &HashMap::new()).unwrap();
    let map: HashMap<String, String> = [("libdep.so".to_string(), dep_new.clone())].into();
    let game = elf::rewrite(&fs::read(dir.join("libgame.so")).unwrap(), Some(&game_new), &map).unwrap();
    fs::write(dir.join(&dep_new), &dep).unwrap();
    fs::write(dir.join(&game_new), &game).unwrap();
    let i = elf::info(&game).unwrap();
    assert_eq!(i.soname.as_deref(), Some(game_new.as_str()));
    assert_eq!(i.needed.iter().filter(|n| n.starts_with("libdep")).collect::<Vec<_>>(), vec![&dep_new]);
    (dir.join(dep_new).display().to_string(), dir.join(game_new).display().to_string())
}

#[test]
fn two_generations_load_side_by_side() {
    if Command::new("cc").arg("--version").output().is_err() {
        eprintln!("no C compiler; skipping");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let (g1, g2) = (t.path().join("g1"), t.path().join("g2"));
    fs::create_dir_all(&g1).unwrap();
    fs::create_dir_all(&g2).unwrap();
    build_gen(&g1, 1);
    build_gen(&g2, 2);
    let (d1, l1) = rename(&g1, "11111111");
    let (d2, l2) = rename(&g2, "22222222");
    // readelf must accept the rewritten files.
    for f in [&d1, &l1, &l2] {
        let o = Command::new("readelf").args(["-d", "-l", "-W", f]).output();
        if let Ok(o) = o {
            assert!(o.status.success(), "readelf failed on {f}: {}", String::from_utf8_lossy(&o.stderr));
            let s = String::from_utf8_lossy(&o.stdout);
            assert!(!s.contains("Error"), "{s}");
        }
    }
    let main = t.path().join("main.c");
    fs::write(&main, r#"
#include <dlfcn.h>
#include <stdio.h>
#include <unistd.h>
static void *must(const char *p) { void *h = dlopen(p, RTLD_NOW | RTLD_LOCAL); if (!h) { printf("dlopen %s: %s\n", p, dlerror()); _exit(2);} return h; }
int main(int c, char **v) {
  must(v[1]); void *g1 = must(v[2]);
  must(v[3]); void *g2 = must(v[4]);
  int (*f1)(void) = dlsym(g1, "game_value"), (*f2)(void) = dlsym(g2, "game_value");
  int *(*c1)(void) = dlsym(g1, "game_dep_counter"), *(*c2)(void) = dlsym(g2, "game_dep_counter");
  printf("%d %d %d\n", f1(), f2(), c1() != c2());
  return 0;
}
"#).unwrap();
    cc(&["-o", "main", "main.c", "-ldl"], t.path());
    let o = Command::new(t.path().join("main")).args([&d1, &l1, &d2, &l2]).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(out.trim(), "10 20 1", "each generation must bind to its own libdep copy; got {out}");
    // Control: without renaming, generation 2 silently binds to generation 1's libdep.
    let o = Command::new(t.path().join("main"))
        .args([g1.join("libdep.so"), g1.join("libgame.so"), g2.join("libdep.so"), g2.join("libgame.so")])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "10 10 0", "control should show the stale binding");
}
