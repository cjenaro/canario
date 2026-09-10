// Post-build AppImage patcher — flagless launches on userns-restricted kernels.
//
// Ubuntu >= 24.04 sets kernel.apparmor_restrict_unprivileged_userns=1, so
// Chromium cannot use its namespace sandbox and falls back to the SUID
// helper — which can never be setuid root inside a user-mounted AppImage.
// The app then aborts with a setuid_sandbox_host FATAL before any app code
// runs, so the only working fix is ELECTRON_DISABLE_SANDBOX=1 in the
// environment before the binary starts. CLI flags can't be passed by
// menu/desktop-integration launches, and app.commandLine.appendSwitch()
// executes too late (the first sandboxed child spawns before JS).
//
// electron-builder 26.x always (re)generates its own AppRun — the afterPack
// hook never fires in 26.8.1 — so we patch AFTER the build: extract the
// finished AppImage, inject the export into AppRun right after the shebang,
// and repack reusing the very runtime bytes already embedded in the image
// (found by locating the squashfs superblock), plus mksquashfs from
// electron-builder's appimage toolset with default (gzip) compression —
// the combination electron-builder itself ships.
//
// Note: the repacked image has no embedded blockmap (differential-update
// data). Auto-update falls back to full downloads; harmless for local and
// unpublished builds.
//
// Usage: node scripts/patchAppImage.cjs [path-or-dist-dir]

"use strict";

const { execFile } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const ENV_LINE = "export ELECTRON_DISABLE_SANDBOX=1";

function run(file, args, opts) {
  return new Promise((resolve, reject) => {
    execFile(file, args, { maxBuffer: 1024 * 1024, ...opts }, (err, stdout, stderr) => {
      if (err) {
        err.stderr = stderr;
        reject(err);
      } else {
        resolve(stdout);
      }
    });
  });
}

// Locate the start of the embedded squashfs: the first "hsqs" whose
// following bytes parse as a plausible v4 superblock. Layouts differ
// slightly between squashfs writers (field widths for compression /
// block_log / version), so accept either packing plus a mkfs_time sanity
// check; "hsqs" may also occur incidentally inside the runtime bytes.
function findSquashfsOffset(buf) {
  let idx = buf.indexOf(Buffer.from("hsqs"));
  while (idx !== -1) {
    if (idx + 40 <= buf.length) {
      const mkfsTime = buf.readUInt32LE(idx + 8);
      const timeOk = mkfsTime > 1100000000 && mkfsTime < 2100000000; // 2001..2036
      const compression16 = buf.readUInt16LE(idx + 20);
      const blockLog16 = buf.readUInt16LE(idx + 22);
      const layoutPacked16 =
        compression16 >= 1 && compression16 <= 8 && blockLog16 >= 12 && blockLog16 <= 20;
      const compression32 = buf.readUInt32LE(idx + 20);
      const blockLog32 = buf.readUInt32LE(idx + 24);
      const major = buf.readUInt16LE(idx + 34);
      const layoutPacked32 =
        compression32 >= 1 &&
        compression32 <= 8 &&
        blockLog32 >= 12 &&
        blockLog32 <= 20 &&
        major === 4;
      if (timeOk && (layoutPacked16 || layoutPacked32)) {
        return idx;
      }
    }
    idx = buf.indexOf(Buffer.from("hsqs"), idx + 1);
  }
  return -1;
}

async function main() {
  const target = process.argv[2] || path.join(__dirname, "..", "dist");

  let appImage = target;
  if (fs.statSync(target).isDirectory()) {
    const candidates = fs
      .readdirSync(target)
      .filter((f) => f.endsWith(".AppImage"))
      .map((f) => path.join(target, f));
    if (candidates.length !== 1) {
      throw new Error(`expected exactly one AppImage in ${target}, found ${candidates.length}`);
    }
    appImage = candidates[0];
  }
  appImage = path.resolve(appImage);

  const original = await fs.promises.readFile(appImage);
  const squashfsOffset = findSquashfsOffset(original);
  if (squashfsOffset < 0) {
    throw new Error("could not locate the squashfs superblock — not a valid AppImage?");
  }
  const runtimeData = original.subarray(0, squashfsOffset); // reuse the exact shipped runtime

  const { getAppImageTools } = require("app-builder-lib/out/toolsets/linux");
  const archMap = { arm64: "arm64", armv7l: "armv7l", x64: "x64", ia32: "ia32" };
  const elfArchMachine = original.readUInt16LE(18); // e_machine: 62=x86-64, 183=aarch64, 40=arm
  const archByMachine = { 62: "x64", 183: "arm64", 40: "armv7l", 3: "ia32" }[elfArchMachine];
  const arch = archMap[archByMachine] ? archByMachine : "x64";
  const { mksquashfs } = await getAppImageTools(arch);

  const work = await fs.promises.mkdtemp(path.join(os.tmpdir(), "canario-appimage-"));
  try {
    // Extract (AppImage runtime writes ./squashfs-root next to cwd).
    await run(appImage, ["--appimage-extract"], { cwd: work });
    const root = path.join(work, "squashfs-root");
    const appRun = path.join(root, "AppRun");

    let script = await fs.promises.readFile(appRun, "utf8");
    if (script.includes(ENV_LINE)) {
      console.log(`> patchAppImage: ${path.basename(appImage)} already patched`);
      return;
    }
    const newline = script.includes("\r\n") ? "\r\n" : "\n";
    const lines = script.split(newline);
    if (!lines[0].startsWith("#!")) {
      throw new Error("AppRun has no shebang line");
    }
    lines.splice(1, 0, ENV_LINE, "# injected by canario-app/scripts/patchAppImage.cjs");
    await fs.promises.writeFile(appRun, lines.join(newline), { mode: 0o755 });

    // Repack: same offset, same runtime bytes, mksquashfs defaults (gzip).
    // Build into a temp file and only replace the original on full success,
    // so a crash can never leave a runtime-less squashfs behind.
    const packed = path.join(work, "packed.AppImage");
    const args = [
      root,
      packed,
      "-offset",
      String(runtimeData.length),
      "-all-root",
      "-noappend",
      "-no-progress",
      "-quiet",
      "-no-xattrs",
      "-no-fragments",
    ];
    await run(mksquashfs, args);

    const out = await fs.promises.readFile(packed);
    const image = Buffer.concat([runtimeData, out.subarray(runtimeData.length)]);
    await fs.promises.writeFile(appImage, image, { mode: 0o755 });
    console.log(
      `> patchAppImage: patched ${path.basename(appImage)} (${ENV_LINE}, runtime ${runtimeData.length}B reused)`,
    );
  } finally {
    fs.rmSync(work, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error("patchAppImage failed:", err.message || err);
  process.exit(1);
});
