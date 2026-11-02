import os, shutil, subprocess, sys
sys.path.insert(0, "/var/home/ratnode2/Projects/AndySU/android/tests")
import device_tests as dt
api = sys.argv[1] if len(sys.argv) > 1 else "34"
here = os.path.dirname(os.path.abspath(__file__))
ndk = os.path.expanduser("~/Android/Sdk/ndk/27.2.12479018/toolchains/llvm/prebuilt/linux-x86_64/bin")
cc = os.path.join(ndk, "x86_64-linux-android29-clang")
out = os.path.join(here, "out"); shutil.rmtree(out, ignore_errors=True); os.makedirs(out)
def sh(*a): subprocess.run(a, check=True)
sh(cc, "-shared", "-fPIC", "-O1", "-Wl,-soname,libcounter.so", "-o", f"{out}/libcounter_a.so", f"{here}/counter.c")
shutil.copy(f"{out}/libcounter_a.so", f"{out}/libcounter_b.so")           # identical bytes
os.symlink("libcounter_a.so", f"{out}/libcounter_link.so")
sh(cc, "-shared", "-fPIC", "-O1", "-Wl,-soname,libcounter2.so", "-o", f"{out}/libcounter2.so", f"{here}/counter.c")
sh(cc, "-O1", "-o", f"{out}/probe", f"{here}/probe.c", "-ldl")
sh("cmp", f"{out}/libcounter_a.so", f"{out}/libcounter_b.so")
dt.CONTAINER = True
d = dt.Device(api, 3)
try:
    d.start()
    rd = "/data/local/tmp/r3"
    d.sh(f"rm -rf {rd}; mkdir -p {rd}")
    for f in ["libcounter_a.so", "libcounter_b.so", "libcounter2.so", "probe"]:
        d.adb("push", f"{out}/{f}", f"{rd}/{f}")
    d.sh(f"ln -s libcounter_a.so {rd}/libcounter_link.so; chmod 755 {rd}/probe")
    print("API", d.sh("getprop ro.build.version.sdk").strip(), d.sh("getprop ro.product.cpu.abi").strip())
    print(d.sh(f"cd {rd} && ./probe {rd} 2>&1", check=False))
finally:
    d.stop()
