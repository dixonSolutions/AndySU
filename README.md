# AndySU — Android Self Update

Complete, permission-free self-updates for Android apps that ship outside the app stores. The APK
becomes a small, frozen bootloader. The app itself (dex, native libraries, assets, and the updater
too) is a signed payload fetched from a remote you own, such as a GitHub repo or any static HTTP
host. Payloads apply live or on the next start, and broken updates roll back automatically.

Work in progress. Design: [docs/BRIEF.md](docs/BRIEF.md). Status: [docs/PROGRESS.md](docs/PROGRESS.md).
