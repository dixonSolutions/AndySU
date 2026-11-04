# Live engine swap in Xonotic Touch: design and feasibility (phase 1)

Status: design only, nothing implemented or built. Everything here was read from source on
2026-11-03: AndySU at `fix/first-ci-run` (95765f5) and the shallow clone in `testbeds/xonotic-touch`
(HEAD 028156c), plus the SDL 2.32.10 sources the clone's build cached in
`~/.cache/xt-perf/android-cache/src/SDL2-2.32.10` (read only). Tags used below:
**[read]** verified by reading code, **[assume]** inferred, not verified, **[unknown]** needs an
experiment (section 3).

Naming correction to the brief: the engine library is **`libmain.so`**, not `libdarkplaces.so`.
The CMake target is `add_library(main SHARED ...)` in `android/app/src/main/cpp/CMakeLists.txt`.
Below, "engine library" means `libmain.so` and its renamed form `libmain.<hash8>.so`.

---------------------------------------------------------------------------------------------------

## 1. How Xonotic Touch starts on Android today

**Process and Java side [read]**
- `BootActivity` (launcher) unpacks the slim data (`GameData`), downloads the ~1.16 GB pk3 set on
  first run, then `startActivity(XonoticActivity)` and `finish()` (`BootActivity.java:274-277`).
  `GameData.REQUIRED_PK3S` = `-data.pk3`, `-maps.pk3`, `-music.pk3`.
- `XonoticActivity extends org.libsdl.app.SDLActivity` (SDL 2.32.10 Java glue, copied from the SDL
  tarball into the tree by `scripts/android-build.sh`). It overrides `getLibraries()` →
  `{"SDL2", "main"}` and `getArguments()` → `-xonotic -basedir .. -userdir .. +exec ..` (+ optional
  `<basedir>/xonotic.args`). It declares `static native nativeInputDevices(boolean,boolean,boolean)`.
- `SDLActivity.onCreate` calls `loadLibraries()` (`SDL.loadLibrary` = `System.loadLibrary`, resolved
  through the app class loader) and starts the Java thread `SDLThread` running `SDLMain.run()`.
- `SDLMain.run()` calls `nativeRunMain(getMainSharedObject(), getMainFunction(), args)`.
  `getMainSharedObject()` = `nativeLibraryDir + "/lib" + <last of getLibraries()> + ".so"` =
  `.../libmain.so`; `getMainFunction()` = `"SDL_main"`. **Both are `protected` and overridable**
  (`SDLActivity.java` lines ~243-262). This is the hook the design uses.
- `nativeRunMain` (`SDL_android.c:738-820`) does `dlopen(library_file, RTLD_GLOBAL)`,
  `dlsym("SDL_main")`, calls it with argv[0]="app_process", and **`dlclose`s the library when it
  returns**. When it returns, `SDLMain` calls `mSingleton.finish()`.

**Native side [read]**
- Libraries in the APK (`scripts/android-build.sh`, `android/app/build.gradle`): `libmain.so`
  (all of darkplaces plus statically linked ogg/vorbis/freetype/jpeg; NEEDED: libSDL2.so, GLESv2,
  EGL, OpenSLES, android, log, m, dl, z), `libSDL2.so`, and `libpng16.so` / `libcurl.so`, which the
  engine `dlopen`s by name (`image_png.c`, `libcurl.c`). `jniLibs.useLegacyPackaging = true`.
- `libmain.so` is built `-fvisibility=hidden -flto=thin`, linked with `-Wl,--gc-sections`; the only
  exported symbols are `SDL_main` and the one JNI function. It is **not** linked with
  `--exclude-libs`, so symbols of the static archives (freetype, vorbis, jpeg) may be exported
  [assume, checkable with `llvm-nm -D`].
- Entry: `sys_sdl.c: main()` compiled as `SDL_main` (`-Dmain=SDL_main`) → `Sys_Main()`
  (`sys_shared.c:1198`) → `Host_Init()` (`host.c:386`) → `while(true) Sys_Frame();`.
- **SDL owns the window and the EGL context.** `vid_sdl.c: VID_InitModeGL` (line 2407) calls
  `SDL_CreateWindow(..., SDL_WINDOW_OPENGL|FULLSCREEN|BORDERLESS)` then `SDL_GL_CreateContext`
  (GLES 2.0 profile), then `GL_InitFunctions()` (function pointers via `SDL_GL_GetProcAddress`).
  Statics `window`, `context` live in the engine. `VID_Shutdown` (2586) does
  `SDL_GL_DeleteContext`, `SDL_DestroyWindow`, `SDL_QuitSubSystem(SDL_INIT_VIDEO)`.
  The Android SDL window layer allows exactly one window but permits destroy-then-create
  (`SDL_androidwindow.c: Android_CreateWindow/Android_DestroyWindow`); the Java `SDLSurface`
  persists across it.
- Main loop: `Sys_Frame()` (`sys_shared.c` ~1150-1190): `if (host.state >= host_shutdown)
  { Host_Shutdown(); exit(0); }`, else `Host_Frame()` (events via `Sys_SDL_HandleEvents`, `Cbuf`,
  `SV_Frame`, `CL_Frame`) then sleep. Quitting always ends in `exit(0)`; `Sys_Error` ends in
  `VID_Shutdown(); exit(1)` (`sys_shared.c:731-777`).
- `Host_Shutdown` (`host.c:584`): `CL_Shutdown` (disconnect, `MR_Shutdown`, `S_Terminate`,
  `R_Modules_Shutdown` = all GL object release, `VID_Shutdown`, free client pools), stop server
  thread, `Host_SaveConfig(config.cfg)` (archived cvars and binds), `Curl_Shutdown`,
  `NetConn_Shutdown`, `TaskQueue_Shutdown`, `Thread_Shutdown`, `Cmd_Shutdown`,
  **`Sys_SDL_Shutdown()` = `SDL_Quit()`**, `Log_Close`, `Crypto_Shutdown`, `Host_UnlockSession`,
  `Con_Shutdown`, `Memory_Shutdown`.
  **What it does NOT do:** no `FS_Shutdown` (pk3 file descriptors stay open), `Memory_Shutdown`
  has its pool frees commented out, no `Mod_Shutdown`/`PRVM` teardown, no free of the dlopen'd
  libpng/libcurl handles. The engine is written to exit right after, so everything the old copy
  leaks stays leaked in a swap (section 3, R3).
- Engine statics are everywhere (`host`, `cls`, `sv`, cvar list, `vid_commandlinecheck`, ...), so
  `Host_Init` cannot be run twice in one copy [assume, consistent with the above]. A second,
  fresh copy of the library is the only safe re-init. This is what the brief's uniquely named
  `.so` gives us.
- The only JNI export in the engine is `Java_..._XonoticActivity_nativeInputDevices`
  (`vid_touchdetect.c:378`), which sets `static int vid_android_state`. Java only re-reports on
  change (`reportedInputState`), so a fresh engine copy would never get it unless Java is told to
  re-send.
- Other threads: server thread (`SV_StartThread`), `TaskQueue`/`Thread_Init` workers, SDL audio
  callback thread (`snd_sdl.c`), curl. All are stopped by the shutdown path above [read, not run].
- Signals: `Sys_InitSignals` (`sys_shared.c:1100`) installs `Sys_HandleCrash` for SIGSEGV/ILL/ABRT/
  FPE/BUS, which replaces the marker-writing handler AndySU installed (`andysu_host.c:
  installCrashHandler`) [assume; order of install not checked on device].

**How AndySU runs it today [read]**: transparent mode (D21). `AndySUComponentFactory.
instantiateClassLoader` → `Transparent.boot` picks the slot, builds a `PayloadClassLoader` over the
slot's dex, sets `ApplicationInfo.nativeLibraryDir` to `slots/<id>/lib/<abi>`. `andysu pack
--no-rename` keeps `libmain.so` etc. Updates apply on the next start only
(`AndySU.onStaged`: `policy == "live" && transparent` → `config.liveFallback`).

**Important finding about the existing test [read + assume]**: `android/tests/testbed_tests.py`
only waits for AndySU's own `booted` event (emitted from `InitProvider` before `Application.
onCreate`). `BootActivity` blocks on the 1.16 GB download and the emulator has no internet, so
the engine probably **never started** in that test. That would explain the black Xonotic
screenshots noted in `docs/PROGRESS.md`. Before swap work: confirm with `adb logcat -s SDL
XonoticTouch` whether `SDL_main` ran. Section 4 seeds real data to fix this.

---------------------------------------------------------------------------------------------------

## 2. Design

### 2.1 Options considered

(a) **Engine as an AndySU stub-mode native payload** on the host's `NativeView` (GLSurfaceView,
`andysu_payload_v1`). Rejected: the engine creates its own window/context through SDL and runs its
own loop (`while(true) Sys_Frame()`), while `NativeView` expects a per-frame callback on a host GL
thread; porting means replacing `vid_sdl.c`, input, audio and the Java side (SDLActivity),
i.e. a different port. It also breaks transparent mode, which exists precisely because the
Activity must stay an `SDLActivity` (D21).

(b) **SDL and its Java glue stay; a small resident loader owns the SDL thread's `main`; the engine
becomes a swappable library behind a one-function ABI.** Chosen. SDL (`libSDL2.so`) is one copy
that all engine copies share, so SDL's global state (Android JNI glue, audio device layer, event
queue, Java `SDLSurface`) survives by construction.

(c) Keep the GL context across the swap (SDL window and `SDL_GLContext` handed from old to new
engine). Better UX (no EGL teardown), but needs GL state hygiene and more engine edits. Kept as
**stage 2** of (b); stage 1 uses the engine's existing window re-creation path (what
`vid_restart` does), which is a proven engine code path.

(d) Swap in a second process. Out of scope (the brief wants same PID).

### 2.2 Architecture of (b)

```
Java:  XonoticActivity (SDLActivity)            AndySU host (frozen)
         getMainSharedObject() -> libandysu_xonotic_loader.so      Host.setLiveSwapHandler(..)
         getMainFunction()     -> "AndySU_SDL_main"                AndySU.applyLiveTransparent()
         EngineSwap.request(path) [JNI] <------------------------- handler.onLiveUpdate(slot,..)

SDLThread:  nativeRunMain -> dlopen(loader) -> AndySU_SDL_main(argc, argv)      [loader, resident]
              loop:  eng = android_dlopen_ext(path, RTLD_NOW|RTLD_LOCAL[,FORCE_LOAD])
                     rc  = eng->run(&loader_api, argc, argv, swapped_in)         [engine copy N]
                     rc == SWAP -> pre-start Java hook (data unpack), path = next, continue
                     rc == QUIT -> exit(0) (unchanged behaviour)
libSDL2.so: single copy, shared by every engine copy.
```

New library **`libandysu_xonotic_loader.so`** (~250 lines C, new CMake target next to `main`,
staged in the APK like `libSDL2.so`). It is kept **unrenamed** and is never swapped live; if a
release changes it, the new copy is used after the next restart (`loader_abi` check below makes
the host refuse a live swap that needs a newer loader).

**Engine ABI** (header `andysu_swap.h`, shared by loader and engine, versioned):

```c
#define ANDYSU_ENGINE_ABI 1
enum { ANDYSU_RUN_QUIT = 0, ANDYSU_RUN_SWAP = 1, ANDYSU_RUN_FAILED = 2 };
typedef struct andysu_loader_api {
    uint32_t abi, size;
    int   (*swap_requested)(void);                 /* atomic read; engine polls once per frame */
    void  (*put_state)(const char *p, size_t n);   /* engine -> next engine (copied by loader) */
    const char *(*get_state)(size_t *n);           /* NULL on cold start */
    void  (*mark_healthy)(void);                   /* -> Java -> Host.current().markHealthy() */
    void  (*emit)(const char *json);               /* -> logcat tag AndySU-Event */
    void  (*abort_start)(const char *why);         /* trial only: longjmp back to the loader */
} andysu_loader_api;
typedef struct andysu_engine {
    uint32_t abi, size, java_abi;                  /* java_abi: JNI contract with the running dex */
    const char *build_id;
    int  (*run)(const andysu_loader_api*, int argc, char **argv, int swapped_in);
    void (*input_devices)(int touch, int keyboard, int mouse);   /* replaces the JNI export */
} andysu_engine;
__attribute__((visibility("default"))) const andysu_engine *andysu_engine_v1(void);
```

The engine library exports **only** `andysu_engine_v1` (version script + `-Wl,--exclude-libs,ALL`)
and is loaded `RTLD_LOCAL`, so two copies never see each other's symbols. This matters because
`nativeRunMain` uses `RTLD_GLOBAL` today: with that, a second copy's references to
default-visibility static-archive symbols (freetype/vorbis/jpeg) could bind to the first copy's.
The loader replaces that path, so the engine is never `RTLD_GLOBAL`.

### 2.3 Exact changes

**A. Xonotic Java (patch `0002-andysu-engine-swap-java.patch`)**

`XonoticActivity.java`:
```java
@Override protected String[] getLibraries() { return new String[]{"SDL2", "andysu_xonotic_loader"}; }
@Override protected String getMainSharedObject() { return getApplicationInfo().nativeLibraryDir + "/libandysu_xonotic_loader.so"; }
@Override protected String getMainFunction() { return "AndySU_SDL_main"; }
@Override protected void onCreate(Bundle b) {          // before super.onCreate starts SDLThread
    EngineSwap.setInitialEngine(EngineSwap.findEngine(getApplicationInfo().nativeLibraryDir)); // libmain.<hash>.so | libmain.so
    ...
}
/** Loader -> Java, on the SDL thread, between old engine's return and new engine's start. */
static void onBeforeEngineStart(String slotDir, boolean swapped) { if (swapped) new GameData(app).prepare(...) }
static void onEngineHealthy() { AndySUCompat.markHealthy(); }
```
New `EngineSwap.java`: `static native boolean request(String enginePath, int javaAbi)` (loader JNI),
`registerHandler()` which calls `com.andysu.api.Host.setLiveSwapHandler(...)` by reflection
(same pattern as `AndySUCompat`, so the same sources still build and run without AndySU).
The handler's `onLiveUpdate(newSlot, from, to)`: locate `lib/<abi>/libmain.*.so` in `newSlot`,
call `EngineSwap.request`, then re-send the input state (`reportedInputState = -1;
reportInputDevices()`) once the loader signals the new engine is up. `onLiveRollback(oldSlot, reason)`
requests the old engine path.
`nativeInputDevices` becomes a Java method that forwards to `EngineSwap.inputDevices(..)`
(loader forwards to `current_engine->input_devices`). The JNI export in `vid_touchdetect.c` is
replaced by a plain function (keep the JNI symbol under `#ifndef ANDYSU_SWAP` for stock builds).

**B. Xonotic native (patch `0003-andysu-engine-swap-native.patch`, option `-DXONOTIC_ANDYSU_SWAP=ON`)**

`CMakeLists.txt`: add `add_library(andysu_xonotic_loader SHARED andysu_loader.c)` (links `log dl`);
for `main`: define `ANDYSU_SWAP`, add `andysu_engine.c`, link options `-Wl,--exclude-libs,ALL
-Wl,--version-script=andysu_engine.map` (exports only `andysu_engine_v1`; keep `SDL_main` exported
when the option is off).

`sys_sdl.c`:
```c
void Sys_SDL_Shutdown(void) {
#ifdef ANDYSU_SWAP
    if (andysu_swapping) return;       /* SDL stays alive: it is shared with the next engine */
#endif
    SDL_Quit();
}
```
`sys_shared.c`:
```c
int Sys_Main(...) { ...; Host_Init(); Andysu_AfterInit();                 /* restore state */
    while (!sys_leave_to_loader) Sys_Frame(); return sys_exit_code; }
Sys_Frame(): at the top
    if (andysu_loader && andysu_loader->swap_requested() && host.state == host_active)
        { Andysu_CaptureState(); andysu_swapping = true; host.state = host_shutdown; }
  and in the host_shutdown branch:
    Host_Shutdown();
    if (andysu_swapping) { sys_exit_code = ANDYSU_RUN_SWAP; sys_leave_to_loader = true; return; }
    exit(0);
Sys_Error(): if (andysu_in_trial) andysu_loader->abort_start(string);   /* before VID_Shutdown/exit */
Sys_InitSignals(): under ANDYSU_SWAP do not install the crash handlers (keep AndySU's).
```
`host.c: Host_Shutdown`: `if (andysu_swapping) { FS_Shutdown(); free the remaining pools }`
(the leak fixes; audit list in R3). Gate: all new calls are no-ops when `andysu_loader == NULL`.

`andysu_engine.c`: `andysu_engine_v1()`, `run()` (stores loader api, `Sys_Main(argc, argv)`),
`Andysu_CaptureState()` / `Andysu_AfterInit()`.

**C. State carried across the swap**

Persistent settings need nothing: `Host_Shutdown` already runs `Host_SaveConfig(config.cfg)`
(archived cvars, binds), and the next `Host_Init` re-reads it through the existing `+exec` chain.
Session state goes through `put_state`/`get_state` as a small JSON, versioned:
```json
{"v":1,"swap_from":"<build_id>","connect":"<ip:port or null>","map":"<name or null>",
 "gametype":"dm","menu":true,"vid_touch_mode":"auto"}
```
- Remote game: `connect` from `cls.netcon->peeraddress` (via `NetConn_AddrToString`) when
  `cls.state == ca_connected`. Local/listen game: `map` from `sv.worldbasename` plus the gametype
  cvar. Menu open/closed from `key_dest`.
- Restore, in `Host_Init` before the existing `togglemenu 1` block: `Cbuf_AddText("connect <addr>\n")`
  or `"map <name>\n"` and execute; this sets `cls.connect_trying`, so the existing "show menu if
  not connecting" branch is skipped. If the saved state was the menu, nothing is queued.
- **Not carried**: QuakeC world state, scores, entity state, a demo in progress. A remote match is
  left and rejoined; a local match restarts the map. This is inherent to the engine and should be
  stated in the user-facing doc. The swap is "a few seconds and rejoin", not a 100 ms freeze.
- Data: slim data (QuakeC progs, menu skin) must match the engine. `GameData.prepare()` is
  re-run from `onBeforeEngineStart` after the old engine is gone and before the new one starts,
  reading the new slot's `xonotic-slim-data.zip` (see host change H3). pk3s in `basedir/data`
  are not touched by an update.

**D. GL resources and SDL global state**

Stage 1 (default): no special handling. Old engine: `CL_Shutdown` → `R_Modules_Shutdown`
(GL objects deleted) → `VID_Shutdown` (`SDL_GL_DeleteContext`, `SDL_DestroyWindow`,
`SDL_QuitSubSystem(VIDEO)`), exactly the sequence `vid_restart` uses (`vid_shared.c:1548`).
New engine: `Host_Init` → `CL_StartVideo` → `VID_InitMode` → `SDL_InitSubSystem(VIDEO)`,
`SDL_CreateWindow`, `SDL_GL_CreateContext`, `GL_InitFunctions`. The Java `SDLSurface` and its
`Surface` persist, so I expect (not verified) the last frame to stay visible during the gap.
SDL stays initialised because `Sys_SDL_Shutdown` skips `SDL_Quit()` while swapping and
`Sys_SDL_Init`'s `SDL_Init(0)` is reference counted (`sys_sdl.c:52`). `SDL_Quit` would also clear
SDL's hints and tear down timers/events. The engine re-sets its hints in `VID_InitModeGL`
(`SDL_HINT_ORIENTATIONS`) and `VID_SetHints_c`.
`libSDL2.so`'s own JNI registration (`nativeSetupJNI`/`RegisterNatives` table in `SDL_android.c`)
is untouched.

Stage 2 (optional, only if stage 1's black gap or EGL churn is a problem): the loader keeps
`SDL_Window*`/`SDL_GLContext` in a shared struct; swap-mode `VID_Shutdown` skips destroying them;
swap-mode `VID_InitModeGL` adopts them (`SDL_GL_MakeCurrent`, `GL_InitFunctions`). Needs a
`GL_ResetToDefaults()` in the old engine's swap shutdown (bind FBO/program/buffers 0, disable
vertex attribs 0-15, blend/depth/cull/scissor/stencil off, active texture 0, unbind all units)
because DarkPlaces' backend caches state and assumes a fresh context. This is the brief's
"GL objects released, context kept".

**E. Avoiding clashes between two engine copies**

1. Unique file and SONAME: `andysu pack` must rename **only** `libmain.so` →
   `libmain.<hash8>.so` (SONAME rewritten; `DT_NEEDED` rewrite is a no-op because nothing needs
   libmain). `libSDL2.so`, `libpng16.so`, `libcurl.so` and the loader keep their names (they must be
   single shared copies; the engine `dlopen`s libpng/libcurl by plain name). Today pack is
   all-or-nothing (`--no-rename`, `cli/src/pack.rs:374`), see host change H1.
2. `RTLD_LOCAL` + single exported symbol, so no cross-binding between copies (above).
3. Per-copy TLS and statics come free with a separate mapping. Per-process resources are the
   issue and are released by the shutdown path: threads (joined), audio device (`S_Terminate`),
   sockets (`NetConn_Shutdown`), pk3 fds (**needs `FS_Shutdown`**, added), libpng/libcurl handles
   (refcounted `dlopen`, harmless).
4. For rollback (re-running the *previous* build, which is already mapped and dirty) use
   `android_dlopen_ext(path, RTLD_NOW|RTLD_LOCAL, {ANDROID_DLEXT_FORCE_LOAD})` to get a fresh
   mapping of the same bytes. Fallback if FORCE_LOAD does not behave: ship a byte-identical twin
   with another name/SONAME in each payload (`libmain.<hash>.r.so`). See R2.
5. Old copies are never `dlclose`d (matches BRIEF 1.3.5); each swap leaves one mapped copy
   (~30 MB virtual, mostly file-backed) plus whatever it leaked. Policy: after K=3 swaps the host
   prefers a cold start the next time the app is backgrounded.

**F. AndySU host/API changes (transparent mode; additive)**

- **H1 `andysu pack`/`split --transparent`**: new `--rename <lib>[,<lib>]` (rename only these) or
  `--keep <lib>[,..]`; `split --transparent` defaults to renaming nothing unless a
  `[native] live_swap = ["main"]` entry exists in `andysu.toml`. `libmain` keeps being found by
  `PayloadClassLoader.findLibrary` (map key "main" → hashed path; verified in `PayloadClassLoader.
  kt`/`Transparent.kt`: `libs[name.removePrefix("lib")]`). Rust diff ~30 lines plus a pack test.
- **H2 API (`android/api`)**: `LiveSwapHandler` interface + `Host.setLiveSwapHandler(..)` /
  `Host.liveSwapHandler()`; Host API level 2 (additive, per BRIEF 1.1). Payloads under an older
  host just never get called (the Xonotic code registers by reflection).
  ```java
  public interface LiveSwapHandler {
      /** Main thread. The staged release was promoted to TRIAL; newSlot is its directory.
       *  Return true if the app now runs it and will call markHealthy(); false/throw = refuse. */
      boolean onLiveUpdate(File newSlot, String fromVersion, String toVersion) throws Exception;
      /** The host rolled the swap back (watchdog or failure report). */
      void onLiveRollback(File oldSlot, String reason);
  }
  ```
- **H3 host `AndySU.kt` / `Transparent.kt`**: in `onStaged`, when `policy == "live" && transparent`
  and a handler is registered, call new `applyLiveTransparent()` instead of `config.liveFallback`:
  `Core.beginLive` (promote to TRIAL, as `applyLive` does) → `Transparent.switchSlot(newDir)`
  (update `slotDir`, `version`, `ApplicationInfo.nativeLibraryDir`; replace the `ResourcesLoader`
  on app and resumed-activity `Resources` via `removeLoaders/addLoaders` so `getAssets()` sees the
  new slot's assets; `TransparentCallbacks.loader` becomes a `var`) → `handler.onLiveUpdate`.
  `false`/throw → `Core.reportFailure` + `switchSlot(old)`. `true` → `armWatchdog(live=true)`;
  commit by the handler's `markHealthy()` or the existing `auto_healthy_ms` timer. `liveRollback`
  gets a transparent branch calling `handler.onLiveRollback`, else `Phoenix.relaunch`.
  Emits `live_swap_requested`, `live_swapped`, `live_rollback` events (same names as stub mode).
- **H4 slot GC**: while a live-swapped process runs, the slot it booted from must not be deleted
  (lazily loaded dex/classes and any not-yet-loaded library still resolve under it). Pass a
  `keep` set (the boot slot) to `Core.gc`, or defer `gcAsync` in transparent live mode until
  restart. Need to check whether `Core.gc` already takes this; I did not read `core/src/store.rs`.
- **H5 engine-only scope**: the Java side (dex) stays at the booted version until restart; only
  the engine, assets and data move. A release is live-swappable only if
  `engine.java_abi == running java_abi` (and `loader_abi` ok). Otherwise the handler returns false
  and the host falls back to `live_fallback` (next start / restart). Document this in `spec/`.
  A release that also changes dex is committed as current but the process keeps running the old
  Java until the next start; that is consistent because every cold start loads Java from the
  current slot.
- No change to the frozen native bridge `andysu_host.c` (the loader is app-side).
  Generalising the loader into the host (usable by STK and any SDL app) is a follow-up.

### 2.4 Rollback

Three layers, cheapest first:

1. **Pre-flight (before touching the old engine)**: the loader `android_dlopen_ext`s the new
   library, resolves `andysu_engine_v1`, checks `abi`, `java_abi`, `size`. Failure → return false
   to Java → host `reportFailure` → old engine never stopped. Catches missing `DT_NEEDED`, corrupt
   ELF, wrong ABI/alignment, wrong symbol. Zero user-visible effect. (Also run in the test as the
   cheap "broken engine" case.)
2. **Start failure in trial (after the old engine was torn down)**: `Sys_Error`/`Host_Error`
   during `Host_Init` of a swapped-in engine calls `abort_start()` → `longjmp` to the loader
   (same thread, loader frame still live) → loader marks the copy poisoned (never reused),
   reloads the previous engine with FORCE_LOAD (or its twin), `run(..., swapped_in=1)` with the
   saved state, emits `engine_rolled_back`, Java calls `Core.reportFailure`. Residue: the failed
   copy's already-started threads (possible: task-queue workers) stay alive, blocked; acceptable
   because it is poisoned [assume; check with thread count].
3. **Crash or hang after start** (SIGSEGV, no frames, no `mark_healthy` within `watchdog_ms`):
   existing AndySU path. Watchdog → `Core.reportFailure` → `Phoenix.relaunch` → cold start on the
   previous slot (about one engine start, seconds). A crash kills the process; the next launch
   counts `bootAttempts` (max 2) and rolls back. For the marker-writing crash handler to be live,
   the swap build must not replace it (`Sys_InitSignals` skipped, see 2.3 B).

`mark_healthy` rule: the engine calls it after the main menu or a map has rendered 180 frames
with no GL error, from the engine thread via loader → Java `Host.current().markHealthy()`.

---------------------------------------------------------------------------------------------------

## 3. Risks and unknowns, ranked

| # | Risk | Likelihood x impact | How to prove/disprove quickly |
|---|---|---|---|
| R1 | **SDL/EGL does not survive window destroy/recreate on a real Android surface** (stage 1), e.g. black surface, `Could not fetch native window`, or SDL video quit loses the Java surface state. Same code path as `vid_restart`, but I did not find proof that it works on Android. | medium x blocks everything | 30 min, no new code: build the **unmodified** engine with +developer, run `vid_restart` from the console on an emulator after the menu is up. Works → stage 1 is viable. Fails → go straight to stage 2 (keep window/context). |
| R2 | Bionic refuses a second mapping (same bytes) or mis-resolves duplicates: affects rollback twin and "unchanged engine hash" swaps. Spec section 9 and D22 assert unique SONAME works; I have not seen `ANDROID_DLEXT_FORCE_LOAD` used in this repo. | medium x rollback only | 15-line test lib + loader test: `android_dlopen_ext` the same file twice with FORCE_LOAD, check distinct handles and distinct static addresses; again with identical SONAME, different path. Run on API 34 and 36 emulators. |
| R3 | **Engine leaks / non-reinit-safe subsystems** beyond what I listed: pk3 fds (certain), pool memory (certain), fonts (freetype static), curl, `Sys_InitProcessNice`, `Host_LockSession` (warns only), `Sys_AllowProfiling`, SDL audio device leftovers. A leak per swap or a crash in the second `Host_Init`. | high x medium | Run the loader + two copies of the *same* engine back to back (cheap: swap to an identical build with FORCE_LOAD) and diff `/proc/self/fd`, `VmRSS`, thread count (`/proc/self/task`) before and after 3 swaps. Do this before any release plumbing. |
| R4 | JNI binding: `nativeInputDevices` and SDL's callbacks resolving to the old (unloaded-in-spirit) copy, or `RegisterNatives` pointing into the old library. Mitigated by moving the one JNI function behind the loader. SDL's own natives live in `libSDL2.so` (single copy). | low x medium | grep `Java_` in the built `libmain` (`llvm-nm -D`) must show only `andysu_engine_v1`; check `adb logcat` for `UnsatisfiedLinkError` after swap. |
| R5 | Audio/input thread state in SDL after `SDL_QuitSubSystem(VIDEO/AUDIO)` and re-init (touch events, `SDL_StartTextInput`, joystick). | medium x low-medium | After a swap: inject a tap with `adb shell input tap` and see the menu react; play a sound (`play misc/menu1`), read `dumpsys audio`. |
| R6 | Existing transparent-mode assumptions: asset `ResourcesLoader` swap works on live `Resources` (API 30+ only); `GameData` re-extraction takes seconds and doubles the freeze; `nativeLibraryDir` is mutated by the host but `dlopen("libpng16.so")` by bare name resolves via the app's *original* linker namespace path, not the mutated field [assume; the current tests do not exercise PNG loading]. | medium x medium | Seed data, run the unmodified transparent flow once and check `adb logcat` for `libpng` load errors; this is also needed to decide whether libpng/libcurl must be loaded by absolute path in the swap build. |
| R7 | Crash in the new engine's first frames kills the process (no live rollback, layer 3 only). Unavoidable; acceptable. | low x low | Test: broken-but-loadable engine (see 4, case C). |
| R8 | Emulator GLES (software/SwiftShader or host GPU) cannot run the engine, so "still rendering" cannot be shown. Existing screenshots were black (unexplained). | medium x medium | Do step 0 of the test plan (run the stock APK with data, read `GL_RENDERER` and frame counters). Use engine-emitted frame counters, not screenshots, as the proof. |
| R9 | Time: first live swap takes 5-15 s with the data re-unpack, vs the brief's "100-500 ms". | certain x cosmetic | Measure; unpack only runs when slim-data version changed (the manifest-based incremental unpack in `GameData` already exists). |

---------------------------------------------------------------------------------------------------

## 4. Test plan (extends `android/tests/testbed_tests.py`)

New bed `xonotic-live`, run on a single emulator (API 34 first, then 36), after steps 0-2.

**Step 0, prerequisite checks (no AndySU involved).** Stock APK with seeded data runs to the main
menu on the emulator. Seeding: `adb push` the pk3s (`xonotic-*-data.pk3`, `-maps`, `-music`, fonts)
from a host copy of `~/.cache/xt-perf/base/data` (that is another project's cache: copy, never
write there) into `/sdcard/Android/data/<pkg>/files/data/` and `xonotic.args` with `+developer 1`.
Needs ~1.2 GB free on the emulator. Fix the existing bed in the same change so it also proves the
engine ran (see "engine-ran" below).

**Observables (all logged by the loader/engine through `emit`, tag `AndySU-Event`, same channel
`device_tests.Device.watch()` already parses):**
```
{"event":"engine_start","pid":P,"lib":"libmain.<h>.so","build_id":"...","base":"0x...","swapped":0|1}
{"event":"engine_frames","pid":P,"lib":...,"frames":N,"gl_renderer":"..."}      every 60 frames
{"event":"engine_swap_begin","pid":P,"from":"...","to":"..."}
{"event":"engine_stopped","pid":P,"lib":...,"fds":F,"rss_kb":R,"threads":T}
{"event":"engine_rolled_back","pid":P,"reason":"..."}
```
and from the host: `live_swap_requested`, `live_swapped`, `committed`, `live_rollback`.
"Library loaded" is also checked independently of the engine: `adb shell run-as <pkg> cat
/proc/<pid>/maps | grep libmain` must list `libmain.<hashA>.so` **and** `libmain.<hashB>.so`, with
the pid unchanged.

**Case A, live swap.**
1. Build engine A (`VCREVISION` tag `andysu-a`) and engine B (`andysu-b`; different bytes ->
   different hash). Split A into host + built-in payload with `split --transparent
   --rename main`; pack B; publish; `apply = "live"`.
2. Launch (BootActivity -> XonoticActivity), wait for `engine_start(lib=A)` and `engine_frames`
   >= 120. Record PID, `maps`, fd count.
3. Staged update arrives; host fires `live_swap_requested`. Assert, in order: `engine_stopped(A)`,
   `engine_start(lib=B, swapped=1)`, then `engine_frames(lib=B)` >= 120 (the game is rendering
   again), `committed`.
4. Assert: same PID before and after (`pidof`), `/proc/<pid>/maps` has both libs, installed
   `versionName` unchanged, `fds` and `threads` after <= before + a small bound (3 swaps in a
   loop: no growth > bound), screenshot saved (informational only, R8).
5. State carry: before the swap `adb shell am broadcast`/console command sets `connect` to a local
   dedicated server started in the emulator host network (or `map` in a local game); after the
   swap assert the `connect`/`map` event from `Andysu_AfterInit` and a frame count with the map
   loaded.
6. Input still works: `adb shell input tap` on the menu; engine emits `touch_event` count.

**Case B, rollback (three cuts, one per layer in 2.4).**
- B1 *unloadable*: payload whose `libmain.<h>.so` has a bogus `DT_NEEDED`. Expect
  `live_swap_requested`, then loader pre-flight failure, **no `engine_stopped`**, old engine keeps
  emitting frames, same PID, `rolled_back` in the host state (release marked bad).
- B2 *fails in Host_Init*: engine built with `-DANDYSU_TEST_FAIL_INIT` calling `Sys_Error("test")`.
  Expect `engine_stopped(A)`, `engine_start(B)`, `engine_rolled_back`, `engine_start(A', swapped=1)`
  and frames again, **same PID**, `maps` shows a third mapping (A's FORCE_LOAD twin).
- B3 *crashes after start*: engine B kills itself with SIGSEGV after 60 frames. Expect process
  death, one relaunch (`relaunch` event), `launch` event with the previous slot, engine A frames
  again, **new PID**. Mirrors the existing `broken` flow in `bed()`.

**Wiring**: add `BEDS["xonotic-live"]` with `engine_a`/`engine_b` APK/payload paths (built by the
steps in section 5, not downloaded) and a `live_bed()` next to `bed()`; extend `Device.watch()`
parsing is unchanged. Add `--only xonotic-live`. Also add an engine-ran assertion to the existing
`bed()` for the unmodified Xonotic (needs seeded data; otherwise skip with a message instead of
passing silently).

---------------------------------------------------------------------------------------------------

## 5. Effort, build steps, ordering

**Order of work (each step ends in an emulator observation):**
1. R1 and R2 experiments (half a day): `vid_restart` on stock engine, FORCE_LOAD test lib.
2. R3: loader + engine ABI + swap to an identical copy, 3 times, measure fds/RSS/threads (1-2 days,
   engine patch 0003 without any AndySU involvement, driven by an `adb` broadcast or a console
   command `andysu_swap <path>`).
3. Java patch 0002 and AndySU H1-H5 (1-2 days; H1 and H2 have unit tests, H3 is covered by the
   new emulator bed).
4. Rollback layers B1-B3 and `put_state` restore (1 day).
5. Optional stage 2 (keep GL context) only if R1 or UX demands it (1-2 days).
Estimate: **5-8 working days** for stage 1 including bed and docs, +2 for stage 2. Largest
uncertainty: R3 (engine teardown completeness).

**Where to work.** `testbeds/xonotic-touch` is the shared shallow clone: read-only for me, and the
patches never get pushed. Build from a scratch copy (131 MB, `testbeds/` is gitignored):
```
cp -a testbeds/xonotic-touch testbeds/build/xonotic-andysu          # not a git worktree
cd testbeds/build/xonotic-andysu
git apply ../../patches/xonotic-touch/0001-*.patch ../../patches/xonotic-touch/0002-*.patch ../../patches/xonotic-touch/0003-*.patch
```
**Toolchain (read from the repo and machine):**
- NDK: CI uses `27.3.13750724` (`.github/workflows/android.yml`, `app/build.gradle`); this machine
  has `~/Android/Sdk/ndk/27.2.12479018` (BRIEF). `android-build.sh` reads the version from
  `source.properties` and symlinks it under `$SDK/ndk/`, so 27.2 should work; the native deps
  cache and the engine are built with whichever is used [assume, not tried].
- AGP is `8.7.3` (`android/build.gradle`) -> needs Gradle 8.9-8.x; CI pins **8.11.1**. The machine
  has only Gradle 9.x in `~/.gradle` and AndySU's wrapper is 9.8.2, which AGP 8.7.3 does not
  support. Install Gradle 8.11.1 once (download into `~/.local/opt`) and pass `GRADLE=...`.
- JDK 17 (`~/.local/opt/jdk-17.0.21+9`; CI uses 17), CMake 3.22.1 + ninja from the SDK
  (`~/Android/Sdk/cmake/3.22.1/bin`, put on `PATH`), ImageMagick present, `ccache` absent (optional).
- Native deps (SDL, ogg, vorbis, freetype, jpeg, png, mbedTLS, curl) are built by
  `scripts/android-deps.sh` per ABI and are slow (tens of minutes). A cache already exists at
  `~/.cache/xt-perf/android-cache/{src,prefix}/{x86_64,arm64-v8a}` (another project's agent).
  Copy it to `testbeds/build/android-cache` and use `ANDROID_CACHE_DIR` pointing at the copy;
  confirm it matches the checked-out `scripts/android-deps.sh` versions (SDL 2.32.10 matches the
  cached source dir). Build x86_64 only for the emulator.
- All of it through the lock/cap wrapper from CLAUDE.md:
```
export ANDROID_SDK_ROOT=$HOME/Android/Sdk ANDROID_NDK_ROOT=$HOME/Android/Sdk/ndk/27.2.12479018
export JAVA_HOME=$HOME/.local/opt/jdk-17.0.21+9 PATH=$JAVA_HOME/bin:$ANDROID_SDK_ROOT/cmake/3.22.1/bin:$PATH
export ANDROID_CACHE_DIR=$PWD/../android-cache GRADLE=$HOME/.local/opt/gradle-8.11.1/bin/gradle
echo "org.gradle.jvmargs=-Xmx2g" >> android/gradle.properties   # per CLAUDE.md
free -g                                   # check RAM first
scripts/heavy.sh bash scripts/android-build.sh --abi x86_64 --version 1.2.9001 --code 9001   # engine A
# change the version stamp or a string constant, then:
scripts/heavy.sh bash scripts/android-build.sh --abi x86_64 --version 1.2.9002 --code 9002   # engine B
$GRADLE --stop
```
  (`android-build.sh` already uses `--no-daemon`; the explicit `--stop` is the CLAUDE.md rule.)
  The engine objects are rebuilt from scratch per version stamp (`VCREVISION` is a compile
  definition), which gives different library bytes; with ccache absent expect ~10-20 min per build
  (not measured). Peak memory of the engine compile with `-flto=thin` is the thing to watch under
  the 10 GB cap [unknown].
- Then the bed itself: `scripts/heavy.sh python3 android/tests/testbed_tests.py --only
  xonotic-live` (starts one emulator; shut it down after, per CLAUDE.md).

---------------------------------------------------------------------------------------------------

## 6. Probe results (phase 2a, 2026-11-03)

All runs through `scripts/heavy.sh`, emulators in the podman image, one at a time, shut down after.
Nothing in `testbeds/xonotic-touch` was modified (`git status` clean). Evidence files are in
`docs/evidence/` (`xonotic-engine-log.txt`, `probe-vid-restart-*`).

**Existing bed made honest.** `android/tests/testbed_tests.py` now seeds the game data and
requires proof that the engine ran. Confirmed: the old bed never proved it (section 1 suspicion).
Findings while fixing it:
- Data seeding: the pk3s (`~/.cache/xt-perf/base/data/*.pk3`, 1130 MiB, symlinks into the flatpak
  install, read only) go into `/sdcard/Android/data/<pkg>/files/data/`. **Directories created by
  `adb shell mkdir` are owned by `shell` and the app then cannot create sub-directories in them**
  (`GameData`: "Cannot create .../data/font-dejavu.pk3dir"). The bed therefore launches the app
  once, waits for it to create `files/.bundle-version` itself, force-stops it, and only then
  pushes the pk3s and `xonotic.args` (`+developer 1`, which is what turns on the engine's logcat
  output, tag `app_process`). The seeding launch happens before `adb reverse`, so it cannot stage
  the update. Missing data fails the bed loudly (`XONOTIC_DATA_DIR` overrides the path).
- Engine-ran assertions (per boot: built-in A, updated B, B after rollback): SDL's
  `Running main function SDL_main from library <...>/no_backup/andysu/<slot>/lib/x86_64/libmain.so`
  with the expected slot (factory / `slots/1.2.105`), then `========Initialized=========`, then
  `GL_RENDERER:`. The stock engine runs on the emulator, and the screenshots are no longer black
  (the menu is drawn). Key lines of the passing run
  (`scripts/heavy.sh python3 android/tests/testbed_tests.py --only xonotic --api 34`, exit 0):
  ```
  [engine] 1.2.104 built-in: V/SDL ( 2331): Running main function SDL_main from library /data/user/0/io.github.dixonsolutions.xonotictouch/no_backup/andysu/factory/t-c82a4b2e-923/lib/x86_64/libmain.so | ========Initialized========= | GL_RENDERER: Android Emulator OpenGL ES Translator (Google SwiftShader)
  [engine] 1.2.105 updated: ... (pid 2770) .../no_backup/andysu/slots/1.2.105/lib/x86_64/libmain.so | ========Initialized========= | GL_RENDERER: ...SwiftShader
  [engine] 1.2.105 after rollback: ... (pid 3326, new process) .../slots/1.2.105/lib/x86_64/libmain.so | ========Initialized========= | GL_RENDERER: ...
  ```
- Useful numbers for the live test: engine start to init banner is about 5 s on the KVM emulator
  with SwiftShader; the first-run screens are the Terms of Service and the Welcome dialog.

**R1: does rendering survive `vid_restart`? Yes (on this setup).** Stock, unmodified 1.2.104 APK,
no AndySU, API 36 `google_apis` x86_64 emulator, data seeded.
Command: `scripts/heavy.sh python3 android/tests/testbed_tests.py --probe-vid-restart --api 36`
(it runs `+exec probe.cfg` with `defer` steps: screenshot, `vid_restart`, screenshot).
Evidence: after `vid_restart` the engine logged `VID_Restart: changing from desktop fullscreen
2400x1080 16bpp ... to desktop fullscreen 1024x768 32bpp`, then a **second** `GL_RENDERER` /
`Using GLES2 rendering path` block (window, EGL context and GL state were recreated on the same
Android surface, same PID), no error lines, and the engine's own `screenshot probe-post.png`
(a `glReadPixels` of the frame it rendered after the restart) and an `adb screencap` both show the
fully drawn menu (`docs/evidence/probe-vid-restart-probe-post.png`). So SDL on Android survives
`SDL_GL_DeleteContext` + `SDL_DestroyWindow` + `SDL_QuitSubSystem(VIDEO)` and the matching
re-creation, which is exactly the stage 1 path of 2.3 D. Caveats: SwiftShader emulator, not a
real GPU/driver; the surface kept its size (desktop fullscreen); I did not measure the black gap
or check that the last frame stays visible during the restart. My SurfaceFlinger frame-timestamp
sampling returned zeros and was dropped; the screenshots are the evidence.

**R2 (bionic second copy; the coordinator's "R3"): yes, and the globals are separate.** Test
library with one mutable global, built with the NDK 27.2 clang (`x86_64-linux-android29`), run as
an executable from `/data/local/tmp` through `adb shell` on API 34 (aosp_atd x86_64). Sources:
`testbeds/patches/xonotic-touch/probes/dlopen/` (`scripts/heavy.sh python3 .../run.py 34`).
```
T1 identical bytes, other file name, SAME SONAME  -> distinct handles, globals SEPARATE
T2 symlink to the first file (control)            -> SAME handle (dedup by file identity)
T3 same path again (control)                      -> SAME handle
T4 same path, android_dlopen_ext(FORCE_LOAD)      -> distinct handle, globals SEPARATE
T5 FORCE_LOAD twice                               -> distinct handles, globals SEPARATE
T6 other bytes, other SONAME and file name        -> distinct handle, globals SEPARATE
```
Consequences: the rollback twin can be a plain copy under another file name or an
`android_dlopen_ext(FORCE_LOAD)` of the same path; renaming the SONAME is not even required for
distinct mappings (but keep it, AndySU's contract and the `DT_NEEDED` rewrite already depend on
it). Limits: this ran in the shell's linker namespace, API 34 only, one flat library without
dependencies. Not yet shown: the same inside an app's namespace and with `libSDL2.so` as a shared
`DT_NEEDED`; that comes with the loader prototype (step 2 of section 5).

---------------------------------------------------------------------------------------------------

## 7. Milestone 2a: in-process engine swap, no AndySU host (2026-11-03)

Patch `0003-andysu-engine-swap.patch` (engine) plus the extended `loader/` sources; built by
`build.sh` as before. Test: `scripts/heavy.sh python3 android/tests/testbed_tests.py
--live-swap-probe --apk-a .../XonoticTouch-1.2.9001-x86_64.apk --apk-b .../XonoticTouch-1.2.9002-x86_64.apk
--api 36 [--map afterslime]` (plain APK install, no `andysu split`: 2a needs no host; the loader's
TEST HOOK file `<external files>/andysu-swap-to` triggers each swap; the loader copies the pushed
`.so` to `files/andysu-swap/libmain.<hash8>.<n>.so`, read-only, and pre-flights it).

What was built: `Sys_Frame` polls `loader->swap_requested()` once per frame; the engine saves JSON
state (`swap_count`, `connect` or `map`, `menu`), runs `Host_Shutdown` along the swap path (no
`SDL_Quit`, no `exit`; plus `FS_Shutdown` and `Mem_FreeAllPools`) and `Sys_Main` returns
`ANDYSU_RUN_SWAP`. The loader runs the pre-flighted engine with `swapped_in=1` and the state,
never `dlclose`s the old one. Rollback layer 2 is implemented: `Sys_Error` during a swapped-in
engine's init (after `VID_Shutdown`) longjmps to the loader, which reloads the previous build with
`ANDROID_DLEXT_FORCE_LOAD` and runs it with `swapped_in=2`.

Result (API 36 google_apis, SwiftShader): A->B->A->B all pass, pid unchanged, `engine_stopped` then
`engine_start(swapped=1)` with the right build id, the state carried (`swap_count` 1,2,3; with
`--map afterslime` the local map is reloaded after every swap), init banner and `GL_RENDERER` again,
`engine_healthy` after 180 frames, the engine renders for 30 s afterwards, `/proc/self/maps` shows
one more `libmain.*.so` per swap (2,3,4,5 distinct). Forced init failure on a 4th swap: pid
unchanged, `engine_start_failed` -> `engine_rolled_back` -> `engine_start(swapped=2)` on the
previous build, frames continue. Timing: engine teardown 86-290 ms; swap request to healthy engine
about 16.5-18 s, almost all of it engine init under SwiftShader (not measured on a device).

Resource numbers (steady state, 10+ s after each swap; no map / local map):
```
            before   swap1  swap2  swap3     growth per swap
fds         167      170    172    174       +2 (anon_inode +2 each swap)        / 171 176 179 182, +3 (+1 socket)
threads     32       32     32     32        0                                   / 33 33 33 33
rss_kb      275592   247508 260524 270960    about +10 MB                        / 881848 906956 917328 940656, about +20 MB
```
Known residue, to fix or live with (bounded by "cold start after K swaps"): about 2 `anon_inode`
fds per swap (an epoll/eventfd/inotify that SDL or the engine opens and never closes), with a local
map one more socket per swap, and RSS growth of 10-20 MB per swap (not returned by the engine's
shutdown). A failed start that is rolled back leaves a poisoned copy: +12 fds, +2 threads, +75 MB
in the one measurement. The test bounds growth over three swaps at 12 fds, 3 threads, 120 MB.

Not done (milestone 2b): the AndySU host hooks (`LiveSwapHandler`, transparent-mode slot switch,
assets loader swap, slot GC, `markHealthy` wiring), `andysu pack` renaming only libmain, the
Java-side data re-unpack before the new engine starts, replacing the file-based TEST HOOK, the
`java_abi` policy at the host level, stage 2 (keep the GL context), connect-state restore against a
real server (code exists, only the local map case was exercised), watchdog-driven rollback layer 3.

---------------------------------------------------------------------------------------------------

## 8. Milestone 2b: AndySU drives the swap (D29), 2026-11-03

Patch `0004-andysu-live-swap-handler.patch` (`EngineSwap.java`, `XonoticActivity`, CMake/gradle
`ANDYSU_FAIL_INIT`) plus loader changes. `EngineSwap.register()` (reflection, no compile dependency)
registers a `LiveSwapHandler` through `HostApi.setLiveSwapHandler` when `Host.current()` exists and
`apiLevel() >= 2`. `onLiveSwap` finds `lib/<abi>/libmain*.so` in the new slot and calls the loader's
new JNI entry `EngineSwap.nativeRequestSwap` (copy to a unique read-only name, `dlopen`, ABI and
`java_abi` check; a refusal declines the swap before anything is torn down). The loader reports to
Java through `EngineSwap.onEngineEvent` (started / failed(reason) / healthy -> `Host.markHealthy()`)
and `EngineSwap.beforeEngineStart` (runs `GameData.prepare()` for the new release between the old
engine's teardown and the new engine's start). The file TEST HOOK now only runs when
`<external files>/andysu-swap-hook` exists (the 2a probe creates it).

Test: `scripts/heavy.sh python3 android/tests/testbed_tests.py --live --api 34 --apk-a ...9001 --apk-b
...9002 --apk-fail ...9003 --apk-dex ...9004` (APKs from `build.sh --variants "1.2.9001 1.2.9002
1.2.9003:fail 1.2.9004:dex"`; the `classes.dex` of 9001/9002/9003 is byte-identical, 9004 changes
Java). Result, one emulator run on API 34: A boots built-in through the loader; B published, a check
in the running process (`cmd jobscheduler run -f`) finds it `native_live`, the handler accepts, and
`live_swap_requested -> engine_swap_begin -> engine_stopped -> engine_start(swapped=1) -> live_swapped
-> engine_healthy -> committed` all happen in the same pid with `state.current = B`; F (init always
fails, built with a define, not the hook) gives `engine_start_failed -> engine_rolled_back ->
engine_start(swapped=2) -> live_rollback -> rolled_back`, same pid, `state.current = B`, `bad = [F]`;
a cold start boots B from `slots/B`; D (dex change) is staged with `native_live=false`, no swap events
for 12 s, and the next start boots D.

Not covered by the bed: watchdog expiry on a live trial (relaunch into the previous slot) and a
native crash during the trial (the engine installs its own signal handlers, so AndySU's marker may
not be written; `max_boot_attempts` then decides), a second live swap after a cold start into a
slot (the GC guard keeps the booted slot; reasoned, not run), a real device. (Since covered: 2c in
section 9, the low-storage guard in section 10.)
(The gap that was here, game data left from the failed release after an in-process rollback, is
closed: section 11.)

---------------------------------------------------------------------------------------------------

## 9. Milestone 2c: failure paths of the AndySU-driven swap, 2026-11-03

Patches `0005-andysu-test-variants.patch` (CMake/gradle: `ANDYSU_TEST_DEFS`; `build.sh` variants
`:nohealthy` = ANDYSU_NO_HEALTHY, `:crash` = ANDYSU_CRASH_AFTER_START, a SIGSEGV 60 frames after the
engine started) and `0006-andysu-keep-host-crash-handlers.patch` (engine: `Sys_InitSignals` no longer
installs the engine's own SIGSEGV/ILL/ABRT/FPE/BUS handlers under the loader).
Test: `scripts/heavy.sh python3 android/tests/testbed_tests.py --live-failures --api 34 --apk-a ..9001
--apk-b ..9002 --apk-c ..9005 --apk-nohealthy ..9006 --apk-crash ..9007` (watchdog_ms 30000), exit 0:
- **Watchdog:** W swaps in (`live_swapped`) but never reports healthy -> `watchdog` after 30 s, relaunch
  (reason watchdog), new pid, B boots committed from `slots/B`, engine runs; W is not offered again
  (`up_to_date`, i.e. marked bad).
- **Second live swap after a cold start:** from that cold-booted B, C swaps in live and commits with
  `state current=C previous=B`; the GC after the commit removed only W's slot, B was kept. (The
  emulator is not low on storage, so this shows the booted slot survives, not the low-storage guard.)
- **Native crash:** G swaps in, SIGSEGV 60 frames later: `native_crash` (signal 11), relaunch, new pid, C
  boots committed from its slot and the engine runs; G not offered again.

The gap found (before 0006): the engine's `Sys_InitSignals` replaced the host's crash handler with
`Sys_HandleCrash`, which prints "Engine Crash", shuts the video down and blocks in an SDL message box
(`Sys_SDL_Dialog`, on unless `-nocrashdialog`). The process stayed alive with the dialog, no crash
marker was written, no `native_crash` event: the trial was rolled back only 30 s later by the
watchdog (relaunch, previous slot), so the rollback happened but late and with a crash dialog on
screen. With 0006 the host's handler runs (marker, pipe poke, relaunch from the crash-watch thread)
and the rollback needs one relaunch. No host change was needed. In a plain (non-AndySU) APK the
crash now goes to Android's default handler instead of the dialog.

---------------------------------------------------------------------------------------------------

## 10. Low-storage GC guard, 2026-11-03

`AndySU.gc()` never deletes the slot a live-swapped process booted from. Its dex and its
not-yet-loaded libraries are still read from there until the next cold start. If that slot is
still `previous`, the GC keeps it even under low storage. If it is no longer kept at all, the GC
is skipped. Low storage is faked with `<root>/test_allocatable_bytes` (an `Updater` hook honoured
only on debuggable apps or userdebug/eng images), written over `adb root`.
Test: `scripts/heavy.sh python3 android/tests/testbed_tests.py --low-storage --api 34 --apk-a ..9001
--apk-b ..9002 --apk-c ..9005 --apk-d ..9008 --apk-e ..9007` (evidence: `docs/evidence/low-storage.json`):
- A -> B live, cold restart: B runs from `slots/B`.
- 190 MiB "free", C swaps in live: the GC runs with `low_storage=true` and removes nothing.
  The state is `current=C previous=B`, and both slots are on disk.
- D swaps in live in the same process: `gc_skipped` ("booted slot B is in use"). B, C and D are on disk.
- 100 kB "free", E offered: the pre-download GC is skipped too, the status is `no_space`, and B stays.
- Cold start into D: the same pre-download GC removes C's slot (the normal low-storage rule) and is
  not skipped.

---------------------------------------------------------------------------------------------------

## 11. Game data after an in-process rollback, 2026-11-03

The gap: when the new engine fails (`ANDYSU_RUN_FAILED`) the loader reloads the previous engine and
runs it with `swapped = 2`, but `java_before_start` (the app unpacking the release's game data,
`EngineSwap.beforeEngineStart`) only ran for `swapped == 1`. The previous engine therefore ran on the
failed release's unpacked data until the next cold start.

Fix: loader `java_before_start(swapped)` now runs for 1 and 2 (`loader/andysu_loader.c`), and patch
`0007-andysu-rollback-prepares-data.patch` (`EngineSwap.java`, `GameData.java`) does the work:
- `onLiveSwap` records `fromVersion` (the release running before the swap; the host has not switched
  yet at that point) and `toVersion` before the loader may start the swap.
- Ordering. The loader calls `notify_java(EV_FAILED)` and then `beforeEngineStart(2)` on the SDL thread.
  The host handles `failed(reason)` by posting to the main looper (`onLiveFailed`: report the failure,
  roll the state back, `switchTransparentSlot` = version, native lib dir, asset loader). So the SDL
  thread posts a latch to the same looper and waits: the looper is FIFO, so when it runs the host's
  rollback has finished and `GameData` reads the previous release's assets. Then it checks that the host
  version is `fromVersion` (else it fails, see below). No sleeps; a 10 s limit only turns a stuck main
  thread into the failure path.
- `GameData.prepareAfterRollback(progress, failedVersion)`: if the failed release's unpack finished
  (the `.bundle-version` stamp equals `toVersion`) the manifest it left is exact, so only differing files
  are rewritten; otherwise (its unpack never started or stopped halfway, files of two releases may be mixed)
  the stamp and manifest are deleted and the whole bundle is unpacked again.
- Observable: `AndySU-Event` `game_data_prepared` {swapped, release, data_version, ms}; `data_version`
  is read back from the stamp on disk after the unpack. `game_data_failed` on an error.
- If preparing fails during a rollback: the stamp and manifest are deleted (so the next start unpacks
  everything), the host is asked to relaunch (`HostApi.updates().restartNow()`, a Phoenix relaunch; the
  cold start boots the rolled-back state and re-unpacks) and `beforeEngineStart` returns 1. The loader
  then does not start the previous engine (`engine_start_failed`, `loader_engine_returned`), because it
  must not run on a mix of two releases' data. If the host cannot relaunch, the SDL main just returns
  and the next start fixes the data. Not exercised by the bed (no way to make the unpack fail).
- Without an AndySU host (the 2a probe) `fromVersion` stays null and the rollback just calls `prepare()`.

Marker: all variants bundle the same slim data zip (built from one clone commit; "0 of 469 files
changed" in every unpack), so file contents cannot tell the releases apart. The marker is GameData's
`.bundle-version` stamp, which holds the version of the release whose bundle was last unpacked (written
after a complete unpack). The bed reads `<external files>/.bundle-version` over adb.

Test: `--live` bed, step 3 (the forced init failure of `:fail` in-process rollback):
`scripts/heavy.sh python3 android/tests/testbed_tests.py --live --api 34 --apk-a ..9011 --apk-b ..9012
--apk-fail ..9013 --apk-dex ..9014` (APKs from `build.sh --variants "1.2.9011 1.2.9012 1.2.9013:fail
1.2.9014:dex"`). After B is live the stamp must be B; after F's rollback the stamp must be B, with
`game_data_prepared` swapped=1 data_version=F (F's start) and swapped=2 data_version=B before B's
engine restarts. Result: passes; log order `engine_rolled_back`, `live_rollback`, "Unpacking ... for B",
`game_data_prepared swapped=2` (687 ms). Against the unfixed APKs (9001-9004) the same bed fails at the
new assertion: "after the in-process rollback the unpacked game data belongs to '1.2.9003' (the failed
release is 1.2.9003), expected 1.2.9002". `--live-failures` (watchdog, second swap, native crash) still
passes with the fixed APKs. Not covered: a failing unpack during a rollback (the relaunch path), and a
host that does not switch to the expected version (the IOException path, which for `swapped == 1` makes the
loader roll back).

The same ordering for `swapped == 1`: the host calls `onLiveSwap` on the main thread (`onStaged` is
posted to main; `applyLive` checks the main looper) and switches its slot (`switchTransparentSlot`:
version, native lib dir, asset loader) in the same main-thread task, after the handler returns and after
`beginLive`. The loader's swap flag is set inside `onLiveSwap`, so the SDL thread could reach
`beforeEngineStart(1)` before that switch (only the old engine's teardown, 86-290 ms, used to make that
unlikely). `beforeEngineStart` now waits for the main looper (a task queued while the host's task is
running or pending runs after it) and requires the host version to be `toVersion` before unpacking; a
mismatch is an `IOException` (`game_data_failed`), the loader treats it as a failed start and rolls back.

---------------------------------------------------------------------------------------------------

## 12. README demo video, 2026-11-03

`docs/media/andysu-xonotic-live-update.mp4` (+ `.gif` preview, `-poster.png`): split screen, real AndySU logs on the
left, Xonotic Touch on an API 36 emulator on the right, updating live from a GitHub Pages remote. Everything is made by
`testbeds/patches/xonotic-touch/demo/record.py` (three stages: `site`, `record`, `render`; `--dry-run` uses a local
`andysu serve` + `adb reverse` remote and its own host, and never writes to `docs/media`).

What the video shows, in order: the host APK installed once with release 1.2.9101 built in (blue banner); the
publisher's terminal (the captured `andysu split / pack / publish / verify --blobs` output, replayed with its own
timestamps, long JSON blocks shortened with `[... n more lines]`); a check in the running app (`cmd jobscheduler run -f`
on the host's job, as the test beds do) that finds the signed release on GitHub Pages, downloads about 5.9 MB in the
background and stages it; `live_swap_requested -> engine_stopped -> engine_start(swapped=1) -> live_swapped ->
engine_healthy -> committed`, all in one pid; the banner turning green (release 1.2.9102, `live swaps 1`) while pid
and process uptime keep counting; then proof lines read over adb (same pid, same `versionName`, same `lastUpdateTime`,
same `base.apk` sha256, no installer / permission / unknown-sources screen in the window focus log, `appops
REQUEST_INSTALL_PACKAGES` untouched). The screen is black for a few seconds while the engines swap: that is real.
Wording: "no install-unknown-apps flow, no install prompt, same APK". The host APK does declare
`REQUEST_INSTALL_PACKAGES` (transparent mode keeps the app's own manifest), but nothing ever uses it.

Reproduce (heavy jobs through `scripts/heavy.sh`, one emulator at a time):
```
source scripts/env.sh
scripts/heavy.sh testbeds/patches/xonotic-touch/build.sh --out testbeds/build/apks-demo --variants "1.2.9101:demo 1.2.9102:demo"
python3 testbeds/patches/xonotic-touch/demo/record.py site          # key in ~/.config/andysu/keys/xonotic-demo/, host APK,
                                                                    # site in testbeds/build/xonotic-demo-site/ (publish demo/xonotic/)
scripts/heavy.sh python3 testbeds/patches/xonotic-touch/demo/record.py record     # emulator, screenrecord, logcat, cue
python3 testbeds/patches/xonotic-touch/demo/record.py render --raw testbeds/build/xonotic-demo/raw/real
```
- Patch `0008-andysu-demo-overlay.patch`, build variant `:demo` (`ANDYSU_DEMO_OVERLAY`): the banner top left, menu and
  game: release (`ANDYSU_BUILD_ID`), `pid`, `process up N s` (`CLOCK_BOOTTIME` minus `/proc/self/stat` starttime;
  `/proc/uptime` is not readable by apps on current Android), `live swaps K` (the `swap_count` in the state the loader
  keeps), accent colour from the last digit of the build id (1 blue, 2 green). Other builds are unchanged.
- The host config of the video: `apply live`, `mode auto`, `check_on_open false`, `watchdog_ms 45000`, `host_api 2`,
  manifest `https://dixonsolutions.github.io/AndySU/demo/xonotic/stable/update.json`, signed with a dedicated demo key.
- The emulator is put in airplane mode until the cue. Without that the host's own scheduled job (it fires right after
  the first launch) finds the already published B and swaps it in before the recording starts.
- Engine arguments skip Xonotic Touch's first-run dialogs (`touch_profile_done`, `touch_setup_done`,
  `_termsofservice_accepted`), so both releases show the main menu.

Found while making it, and fixed by patch `0009-andysu-ignore-stale-window-events.patch`: after every live swap
(milestones 2a-2c, 11) the new engine never drew another frame. The beds counted `engine_frames`, which kept counting,
and `live-swap-final.png` shows the screen stuck on the loading plaque. Cause (engine log of `vid_hidden`, `scr_loading`
and every SDL window event, see the gap in the screen recording): the old engine's window is destroyed and hidden, SDL
stays initialised, and the HIDDEN / FOCUS_LOST events of that window are still in SDL's queue; the new engine's window has
the same id (1), so the new engine took the stale `SDL_WINDOWEVENT_HIDDEN` for its own, set `vid_hidden`, and
`SCR_UpdateScreen` returned early forever (no SHOWN event follows). The fix ignores a HIDDEN event while the current
window's SDL flags say SHOWN (SDL has already updated the flags when it posts a real hide). Any APK built before 0009
shows a frozen screen after a live swap; rebuild the beds' APKs to get it.

Result of the recording against the real remote (API 36 google_apis emulator, container mode, 2026-11-04):
`docs/media/andysu-xonotic-live-update.mp4` is 3.10 MB, 69.0 s, 1920x1080, H.264 yuv420p, 15 fps, faststart (copy in
`docs-site/media/` for the docs Introduction page); `.gif` 1.66 MB, 1000x563, 8 fps, 20 s (from just before the swap
request to a few seconds after the commit); `-poster.png` a frame after the commit. Timeline in the video: cue at 17.5 s,
`live_swap_requested` 28.7 s, `live_swapped` 30.1 s, `committed` 45.9 s (about 28 s from the cue to the commit, the
download of 5,915,418 bytes from GitHub Pages included). Proof lines of that run: pid 3634 before and after,
`versionName` 1.2.9101 and `lastUpdateTime` unchanged, `base.apk` sha256 `91ddef0273f9...` unchanged, no installer,
permission or unknown-sources window in the focus log (launcher, then Xonotic's activity only), `appops
REQUEST_INSTALL_PACKAGES` "No operations". Xonotic Touch's own self-updater did not open the unknown-sources screen.
The emulator image has no `curl`; that the emulator reaches the remote is shown by the host's own `update_status`
events for 1.2.9102 (`downloading`, `staged`) fetched from the Pages URL. Left panel notes: the long JSON of
`update_status` and `committed` is shown without its constant fields, the publisher's terminal is the output captured
when the site was built (its timestamps are those of that run), `lib` paths are shortened with `...`.

---------------------------------------------------------------------------------------------------

## 13. On-screen check: frames must reach the screen, 2026-11-04

The beds used to accept "the engine runs" from AndySU events alone (`engine_frames`), which kept
counting while the screen was frozen after a live swap (section 12, fixed by patch 0009). Every
point where a bed accepts a running engine now also calls `screen_alive(d, what)` in
`android/tests/testbed_tests.py`:

- capture the raw framebuffer (`adb exec-out screencap`, no PNG: a 12- or 16-byte header, then
  RGBA; standard library only, sampled every 8th pixel);
- press a key that only a live, drawing engine answers: first `` ` `` (`KEYCODE_GRAVE`, the console
  slides over the top half; it is closed again), then `DPAD_DOWN`, then `DPAD_UP`;
- pass when a capture 1.5 s later is non-uniform (std-dev >= 10) and at least **2 %** of the sampled
  pixels changed; fail after three keys, saving a PNG of the screen to `docs/evidence/`.

Two things made it deterministic: the beds now skip Xonotic Touch's first-run dialogs (profile,
touch setup, terms of service: the README demo's engine arguments, `FIRST_RUN_SKIP`), and mark
Android's "Viewing full screen" bubble as confirmed on their own emulators (`no_immersive_prompt`).
Both had covered the game and swallowed the key presses. The Xonotic beds now default to API 36
`google_apis`, which captures the game's GL frames (ATD images capture them as black).

Results, API 36, APKs rebuilt with patch 0009 (`testbeds/build/apks-0009`), lead's runs:

| Bed | Checks | Change after the key |
|---|---|---|
| `--live` | built-in, live swap, in-process rollback, cold start | 61.7 %, 61.6 %, 92.3 %, 62.0 % (all on the first key) |
| `--live-failures` | built-in, watchdog relaunch, crash relaunch | 62.0 %, 62.4 %, 62.2 % |
| `--low-storage` | built-in, two cold starts | 25.3 % each (run with the earlier DPAD-first key order) |

**Without patch 0009** (A and B rebuilt without it, `scripts/build-without-patch-0009.sh`, which
takes the heavy-job lock itself: do not wrap it in `heavy.sh`), `--live` passes the built-in check
(61.7 %) and then fails exactly at the live swap:
`screen not alive (live-B-swapped): std 15.7/15.7, but only 0.0% of pixels changed after 3 key
presses (need 2%): frozen`. The screen: `docs/evidence/screen-check-frozen-without-0009.png`, the
stuck "Loading models" plaque.

A Haiku worker started this; its versions depended on numpy (not installed), used an unmeasured
threshold and once dropped the change requirement altogether. The lead took it over, rewrote the
check as above and ran every bed.

## 14. AndySU update UI in the live Xonotic Touch checkout (2026-11-04)

The live `~/Projects/Xonotic-Touch` checkout already has the Touch updates dialog; the testbed clone
is older and does not, so this part is kept as a snapshot rather than a numbered patch in `build.sh`.
The snapshot is `testbeds/patches/xonotic-touch/live/andysu-update-ui-8afbabe.diff` (base commit
`8afbabe`, the live checkout's HEAD on 2026-11-04). It covers:

- `AndySUBridge.java` (new): reads `touch/andysu-update-request.txt`, calls the host's
  `UpdateClient` through `AndySUCompat`, and publishes `touch/andysu-update-status.txt` for QuakeC.
- `AndySUCompat.java`: `mode()`, `setMode()`, `checkNow()`, `applyNow()` by reflection. Older hosts
  simply lack the methods; the menu treats that as an incompatible payload API.
- `XonoticActivity.java`: starts the AndySU bridge beside the APK updater; the menu's
  "Update automatically" checkbox changes the host's persistent mode (`auto`/`off`) when AndySU is
  present, and the APK updater preference otherwise.
- `UpdateBridge.java`: reports the same auto value from the host mode, so the checkbox stays
  consistent.
- `touch_update_util.qh/qc` and `dialog_touch_updates.qh/qc`: an "AndySU payload" section with the
  installed payload version, latest release, status, indeterminate progress while busy, and
  "Check payload" / "Apply staged" buttons. The section polls the status file once per second.
- `GameData.java`: the live base has only `.bundle-version` (no `.bundle-manifest`), so
  `invalidateBundle()` deletes the stamp; the testbed base keeps patch 0007's two-file version.

Verification on 2026-11-04: the live checkout built with
`scripts/android-build.sh --abi x86_64 --version 1.2.9999 --code 9999` after the QuakeC
`-Werror` fix at `dialog_touch_updates.qc:270` (parenthesise `managed && !payloadAvailable`).
Native libraries, Java and both `csprogs.dat`/`menu.dat` compiled; the APK is 29 MB. Running the APK
on a device and exercising the dialog is still to do.
