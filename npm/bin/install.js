#!/usr/bin/env node
// Download the treeproof release binary that matches this package version.
"use strict";
const fs = require("fs");
const os = require("os");
const path = require("path");
const https = require("https");
const { execFileSync } = require("child_process");

const pkg = require("../package.json");
const binDir = path.join(__dirname, "native");
const exe = process.platform === "win32" ? "treeproof.exe" : "treeproof";

function target() {
  const key = `${process.platform}-${process.arch}`;
  const map = {
    "linux-x64": ["x86_64-unknown-linux-musl", "tar.gz"],
    "linux-arm64": ["aarch64-unknown-linux-musl", "tar.gz"],
    "darwin-arm64": ["aarch64-apple-darwin", "tar.gz"],
    "darwin-x64": ["x86_64-apple-darwin", "tar.gz"],
    "win32-x64": ["x86_64-pc-windows-msvc", "zip"],
  };
  if (!map[key]) throw new Error(`treeproof: no prebuilt binary for ${key}; install with cargo install treeproof`);
  return map[key];
}

function download(url, dest, redirects = 0) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { "User-Agent": "treeproof-npm" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location && redirects < 10) {
          res.resume();
          resolve(download(res.headers.location, dest, redirects + 1));
          return;
        }
        if (res.statusCode !== 200) {
          res.resume();
          reject(new Error(`treeproof: download failed (${res.statusCode}) ${url}`));
          return;
        }
        const out = fs.createWriteStream(dest);
        res.pipe(out);
        out.on("finish", () => out.close(resolve));
        out.on("error", reject);
      })
      .on("error", reject);
  });
}

async function install() {
  const dest = path.join(binDir, exe);
  if (fs.existsSync(dest)) return dest;
  const [t, ext] = target();
  const url = `https://github.com/Mattbusel/treeproof/releases/download/v${pkg.version}/treeproof-${t}.${ext}`;
  fs.mkdirSync(binDir, { recursive: true });
  const archive = path.join(os.tmpdir(), `treeproof-${process.pid}.${ext}`);
  await download(url, archive);
  // bsdtar ships with Windows 10 and later and extracts zip as well as tar.gz.
  execFileSync("tar", ["-xf", archive, "-C", binDir], { stdio: "inherit" });
  fs.unlinkSync(archive);
  if (process.platform !== "win32") fs.chmodSync(dest, 0o755);
  return dest;
}

module.exports = { install, binPath: () => path.join(binDir, exe) };

if (require.main === module) {
  install().catch((e) => {
    // Never fail `npm install`; the launcher retries on first use.
    console.error(e.message);
  });
}
