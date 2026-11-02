# Test-bed integration patches

Patches for the test-bed projects, applied to local clones only (never pushed to their repos).
AndySU runs both games unchanged from their published release APKs in transparent mode
(`android/tests/testbed_tests.py`); these patches only fix behaviour that depends on the
*installed APK* rather than the running code:

| Patch | Why |
|---|---|
| `xonotic-touch/0001-andysu-payload-version-and-updater.patch` | `GameData` re-unpacks its data bundle when `PackageInfo.versionName` changes; under AndySU that is the frozen host's version, so it asks AndySU for the payload version instead. `AppUpdater` (PackageInstaller APK updates) stands down under AndySU, since installing a full APK would replace the host. |
| `SuperTuxKart-Touch/0001-andysu-updater-stands-down.patch` | `STKUpdateChecker` stands down under AndySU for the same reason. STK keys its data on a compile-time version inside `libmain.so`, which is already the payload's. |

Both use reflection on `com.andysu.api.Host`, so the patched apps build and run with or without
AndySU. Checked: `git apply --check` against the current clones; the patched Xonotic classes
compile with javac against android.jar (only the generated `R` class is missing outside a full
build). They have not been through the projects' full engine builds.
