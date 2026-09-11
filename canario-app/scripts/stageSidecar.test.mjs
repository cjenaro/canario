// Tests for scripts/stageSidecar.cjs (canario-lct): the staging step chained
// before electron-builder in `npm run package:linux`. Builds a throwaway
// repo layout in a tmpdir and drives the exported stageSidecar() directly.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { collectRustInputs, stageSidecar } from "./stageSidecar.cjs";

const BINARY = process.platform === "win32" ? "canario-electron.exe" : "canario-electron";

// Minimum time gap between "old" and "new" fixture mtimes — generous so the
// staleness comparison is robust on filesystems with coarse mtime granularity.
const HOUR_MS = 60 * 60 * 1000;

let root;

function writeFile(rel, content, mtimeMs) {
  const file = path.join(root, rel);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
  if (mtimeMs !== undefined) {
    const t = new Date(mtimeMs);
    fs.utimesSync(file, t, t);
  }
  return file;
}

// Layout: workspace root with one crate, a release binary, and the
// canario-app/sidecar destination directory (created by stageSidecar).
function makeRepo({ binMtime, srcMtime } = {}) {
  const now = Date.now();
  const src = srcMtime ?? now - 2 * HOUR_MS;
  const bin = binMtime ?? now - HOUR_MS;
  writeFile("Cargo.toml", '[workspace]\nmembers = ["canario-core"]\nresolver = "2"\n', src);
  writeFile("Cargo.lock", "# lockfile\n", src);
  writeFile("canario-core/Cargo.toml", '[package]\nname = "canario-core"\n', src);
  writeFile("canario-core/src/lib.rs", "pub fn f() {}\n", src);
  writeFile("canario-core/src/nested/mod.rs", "pub fn g() {}\n", src);
  writeFile(`target/release/${BINARY}`, "fake-binary", bin);
}

beforeEach(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), "canario-stage-sidecar-"));
});

afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

describe("stageSidecar", () => {
  it("copies the release binary into canario-app/sidecar and makes it executable", () => {
    makeRepo();
    const result = stageSidecar(root);

    const dest = path.join(root, "canario-app", "sidecar", BINARY);
    expect(result.staged).toBe(dest);
    expect(fs.readFileSync(dest, "utf8")).toBe("fake-binary");
    if (process.platform !== "win32") {
      expect(fs.statSync(dest).mode & 0o111).not.toBe(0);
    }
  });

  it("fails loudly when the release binary is missing", () => {
    makeRepo();
    fs.rmSync(path.join(root, "target", "release", BINARY));

    expect(() => stageSidecar(root)).toThrowError(/release binary not found/);
    expect(() => stageSidecar(root)).toThrowError(/cargo build --release/);
  });

  it("fails loudly when the binary is older than any Rust source", () => {
    // Binary built first, then a source file touched afterwards.
    makeRepo({ binMtime: Date.now() - 2 * HOUR_MS, srcMtime: Date.now() });

    expect(() => stageSidecar(root)).toThrowError(/older than \d+ Rust build input/);
    expect(() => stageSidecar(root)).toThrowError(/canario-core\/src\/lib\.rs/);
    expect(() => stageSidecar(root)).toThrowError(/cargo build --release/);
    // And nothing was staged.
    expect(fs.existsSync(path.join(root, "canario-app", "sidecar", BINARY))).toBe(false);
  });

  it("detects staleness from the lockfile and crate manifest, not just .rs files", () => {
    makeRepo();
    const newer = Date.now();
    const lock = path.join(root, "Cargo.lock");
    fs.utimesSync(lock, new Date(newer), new Date(newer));

    expect(() => stageSidecar(root)).toThrowError(/Cargo\.lock/);
  });

  it("ignores newer sibling frontend sources that do not rebuild the sidecar", () => {
    makeRepo();
    writeFile("canario-gtk/src/main.rs", "// GTK-only change", Date.now());
    writeFile("canario-cli/src/main.rs", "// CLI-only change", Date.now());
    expect(stageSidecar(root).staged).toBe(path.join(root, "canario-app", "sidecar", BINARY));
  });

  it("rejects newer sidecar sources and preserves an already staged binary", () => {
    makeRepo();
    writeFile("canario-electron/src/main.rs", "// sidecar change", Date.now());
    const dest = writeFile(`canario-app/sidecar/${BINARY}`, "previous-binary");
    expect(() => stageSidecar(root)).toThrowError(/canario-electron/);
    expect(fs.readFileSync(dest, "utf8")).toBe("previous-binary");
  });
});

describe("collectRustInputs", () => {
  it("includes manifests, lockfile, and nested src .rs files, but not tests", () => {
    makeRepo();
    writeFile("canario-core/build.rs", "fn main() {}\n");
    writeFile("canario-core/tests/integration.rs", "// not a build input\n");

    const rel = collectRustInputs(root)
      .map((f) => path.relative(root, f))
      .sort();

    expect(rel).toEqual([
      "Cargo.lock",
      "Cargo.toml",
      "canario-core/Cargo.toml",
      "canario-core/build.rs",
      "canario-core/src/lib.rs",
      "canario-core/src/nested/mod.rs",
    ]);
  });
});
