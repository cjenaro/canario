// Stage the Rust sidecar binary for electron-builder packaging (canario-lct).
//
// electron-builder.yml bundles whatever sits in canario-app/sidecar/ via
// extraResources, but nothing in the local flow refreshed it — CI has a
// manual "Prepare sidecar directory" cp step that local packaging never
// replicated, so `npm run package:linux` silently shipped a stale binary.
//
// This script copies target/release/canario-electron into
// canario-app/sidecar/ and refuses to run when the binary is missing or
// older than any Rust build input (workspace manifests, Cargo.lock,
// build.rs, and every .rs under the sidecar crates' src/), so a stale sidecar
// fails loudly instead of shipping.
//
// CI is unaffected: .github/workflows/build.yml builds the sidecar in a
// separate job, downloads the artifact into canario-app/sidecar/, and calls
// `npx electron-builder` directly — this script never runs there.
//
// Usage: node scripts/stageSidecar.cjs   (chained by `npm run package:linux`)

"use strict";

const fs = require("fs");
const path = require("path");

const BINARY_NAME =
  process.platform === "win32" ? "canario-electron.exe" : "canario-electron";

// The sidecar depends on core, not the sibling GTK/CLI frontends. Keep this
// list aligned with local path dependencies in canario-electron/Cargo.toml.
// Including unrelated frontends can reject a freshly built sidecar forever:
// Cargo correctly does not relink it after a GTK-only edit.
const SIDECAR_CRATES = ["canario-core", "canario-electron"];

function walkRustFiles(dir, out) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      walkRustFiles(full, out);
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      out.push(full);
    }
  }
}

// Every file whose change should produce a fresh release binary: workspace
// + crate manifests, the lockfile, build scripts, and all crate sources.
// (tests/ are excluded — they don't affect the shipped binary.)
function collectRustInputs(repoRoot) {
  const inputs = [];
  const push = (file) => {
    if (fs.existsSync(file)) inputs.push(file);
  };
  push(path.join(repoRoot, "Cargo.toml"));
  push(path.join(repoRoot, "Cargo.lock"));
  for (const member of SIDECAR_CRATES) {
    const crateDir = path.join(repoRoot, member);
    push(path.join(crateDir, "Cargo.toml"));
    push(path.join(crateDir, "build.rs"));
    const srcDir = path.join(crateDir, "src");
    if (fs.existsSync(srcDir)) walkRustFiles(srcDir, inputs);
  }
  return inputs;
}

function stageSidecar(repoRoot = path.join(__dirname, "..", "..")) {
  const releaseBin = path.join(repoRoot, "target", "release", BINARY_NAME);
  if (!fs.existsSync(releaseBin)) {
    throw new Error(
      `stageSidecar: release binary not found: ${releaseBin}\n` +
        "Build it first:  cargo build --release --bin canario-electron",
    );
  }

  const binMtimeMs = fs.statSync(releaseBin).mtimeMs;
  const stale = collectRustInputs(repoRoot)
    .filter((file) => fs.statSync(file).mtimeMs > binMtimeMs)
    .map((file) => path.relative(repoRoot, file))
    .sort();
  if (stale.length > 0) {
    const listed = stale.slice(0, 10).map((f) => `  - ${f}`);
    if (stale.length > 10) listed.push(`  … and ${stale.length - 10} more`);
    throw new Error(
      `stageSidecar: target/release/${BINARY_NAME} is older than ${stale.length} Rust build input(s):\n` +
        listed.join("\n") +
        "\nRebuild before packaging:  cargo build --release --bin canario-electron",
    );
  }

  const sidecarDir = path.join(repoRoot, "canario-app", "sidecar");
  fs.mkdirSync(sidecarDir, { recursive: true });
  const dest = path.join(sidecarDir, BINARY_NAME);
  fs.copyFileSync(releaseBin, dest);
  fs.chmodSync(dest, 0o755);
  return { staged: dest, from: releaseBin };
}

if (require.main === module) {
  try {
    const result = stageSidecar();
    const size = fs.statSync(result.staged).size;
    console.log(
      `> stageSidecar: staged ${result.staged} (${(size / 1024 / 1024).toFixed(1)} MiB, up-to-date with Rust sources)`,
    );
  } catch (err) {
    console.error(err.message || err);
    process.exit(1);
  }
}

module.exports = { stageSidecar, collectRustInputs };
