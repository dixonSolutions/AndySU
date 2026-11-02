#!/usr/bin/env node
// Runs the static andysu binary for this platform, bundled under vendor/ by the release workflow.
"use strict";
const { spawnSync } = require("child_process");
const path = require("path");
const fs = require("fs");

const targets = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
};
const target = targets[`${process.platform}-${process.arch}`];
const bin = target && path.join(__dirname, "..", "vendor", `andysu-${target}`);
if (!bin || !fs.existsSync(bin)) {
  console.error(`andysu: no bundled binary for ${process.platform}-${process.arch}`);
  process.exit(1);
}
try {
  fs.accessSync(bin, fs.constants.X_OK);
} catch {
  fs.chmodSync(bin, 0o755);
}
const r = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
process.exit(r.status === null ? 1 : r.status);
