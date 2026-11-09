#!/usr/bin/env node
// 一条命令发版：改版本号 → 推 main 和 tag（GitHub 开始打 Windows 包）→ 本机同时打 Mac 包、
// 签名、上传 → 两端各自把自己写进 latest.json（见 updater-manifest.mjs）。
//
//   pnpm release 0.39.0          # 正式发版
//   pnpm release 0.39.0 --dry-run  # 只核对并本机打 Mac 包，不改版本、不推送、不上传
//
// 中途失败（比如 Mac 打包出错）修好后用同一个版本号重跑：已推送的 tag 指向当前提交时
// 直接从打包那一步继续，不会重复改版本。
//
// 前提：在 main、工作区干净且与 origin/main 一致；gh 已登录；本机有更新签名私钥
// ~/.tauri/gendesk-signing.key 和钥匙串里的代码签名证书「GenDesk Self-Signed」。

import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { NOTES, add } from "./updater-manifest.mjs";

const say = (text) => process.stdout.write(`${text}\n`);

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO = "indincys/gendesk";
const SIGNING_KEY =
  process.env.GENDESK_SIGNING_KEY ?? path.join(os.homedir(), ".tauri/gendesk-signing.key");
const SIGNING_IDENTITY = process.env.GENDESK_SIGNING_IDENTITY ?? "GenDesk Self-Signed";
const BUNDLE = path.join(ROOT, "src-tauri/target/release/bundle");

/** 带版本号的文件与替换规则；改一处版本号就要改这几处。 */
const VERSION_FILES = [
  ["package.json", /("version":\s*")[^"]+(")/],
  ["src-tauri/tauri.conf.json", /("version":\s*")[^"]+(")/],
  ["src-tauri/Cargo.toml", /^(version\s*=\s*")[^"]+(")/m],
  ["scripts/gendesk.mjs", /(const CLI_VERSION = ")[^"]+(")/],
];

function fail(message) {
  console.error(`✗ ${message}`);
  process.exit(1);
}

function run(cmd, args, { env, capture = false, allowFail = false } = {}) {
  const r = spawnSync(cmd, args, {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, ...env },
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
  if (!allowFail && (r.error || r.status !== 0)) {
    fail(`${cmd} ${args.join(" ")} 失败${capture ? `：${(r.stderr || "").trim()}` : ""}`);
  }
  return { ok: !r.error && r.status === 0, out: (r.stdout || "").trim() };
}

const git = (...args) => run("git", args, { capture: true }).out;

function currentVersion() {
  return JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8")).version;
}

function newer(a, b) {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < 3; i += 1) if (pa[i] !== pb[i]) return pa[i] > pb[i];
  return false;
}

function step(title) {
  say(`\n── ${title} ──`);
}

function checkPreconditions(version, dryRun) {
  step("核对");
  if (!/^\d+\.\d+\.\d+$/.test(version)) fail(`版本号要写成 X.Y.Z：${version}`);
  if (git("rev-parse", "--abbrev-ref", "HEAD") !== "main") fail("先切到 main");
  const dirty = git("status", "--porcelain")
    .split("\n")
    .filter((l) => l && !l.endsWith(".DS_Store"));
  if (dirty.length && !dryRun) fail(`有未提交的改动：\n${dirty.join("\n")}`);
  if (dirty.length) say("⚠️ 有未提交的改动；试发照做，正式发版会被拦下");
  if (!dryRun) {
    run("git", ["fetch", "-q", "origin", "main", "--tags"], { capture: true });
    if (git("rev-parse", "HEAD") !== git("rev-parse", "origin/main"))
      fail("本地 main 与 origin/main 不一致：先 pull 或 push");
    run("gh", ["auth", "status"], { capture: true });
  }
  if (!fs.existsSync(SIGNING_KEY)) fail(`没有更新签名私钥：${SIGNING_KEY}`);
  const identities = run("security", ["find-identity", "-v", "-p", "codesigning"], {
    capture: true,
  }).out;
  if (!identities.includes(`"${SIGNING_IDENTITY}"`))
    fail(`钥匙串里没有代码签名证书「${SIGNING_IDENTITY}」`);
}

/** 返回 true 表示 tag 已在当前提交上（续跑）。 */
function bumpAndTag(version) {
  const tag = `v${version}`;
  const tagged = run("git", ["rev-parse", "-q", "--verify", `refs/tags/${tag}^{commit}`], {
    capture: true,
    allowFail: true,
  });
  if (tagged.ok) {
    if (tagged.out !== git("rev-parse", "HEAD")) fail(`${tag} 已存在且不在当前提交上`);
    say(`${tag} 已在当前提交上，从打包继续`);
    return;
  }
  const from = currentVersion();
  if (!newer(version, from)) fail(`新版本 ${version} 要大于当前 ${from}`);

  step(`改版本号 ${from} → ${version}`);
  for (const [file, pattern] of VERSION_FILES) {
    const full = path.join(ROOT, file);
    const text = fs.readFileSync(full, "utf8");
    if (!pattern.test(text)) fail(`${file} 里找不到版本号`);
    fs.writeFileSync(full, text.replace(pattern, `$1${version}$2`));
  }
  run("cargo", ["update", "-q", "-p", "gendesk", "--manifest-path", "src-tauri/Cargo.toml"]);
  run("git", ["add", ...VERSION_FILES.map(([f]) => f), "src-tauri/Cargo.lock"]);
  run("git", ["commit", "-q", "-m", `chore(release): ${tag}`]);

  step("推送 main 与 tag（GitHub 开始打 Windows 包）");
  run("git", ["push", "-q", "origin", "main"]);
  run("git", ["tag", "-a", tag, "-m", `GenDesk ${tag}`]);
  run("git", ["push", "-q", "origin", tag]);
}

function ensureRelease(tag) {
  // 先建好 Release，Windows 那端的 tauri-action 找到同名 Release 就直接往里传。
  const exists = run("gh", ["release", "view", tag, "--repo", REPO], {
    capture: true,
    allowFail: true,
  });
  if (exists.ok) return;
  const created = run(
    "gh",
    [
      "release",
      "create",
      tag,
      "--repo",
      REPO,
      "--verify-tag",
      "--title",
      `GenDesk ${tag}`,
      "--notes",
      NOTES,
    ],
    { capture: true, allowFail: true },
  );
  // 两端同时建会撞上，对方建好了就用对方的。
  if (
    !created.ok &&
    !run("gh", ["release", "view", tag, "--repo", REPO], { capture: true, allowFail: true }).ok
  ) {
    fail(`建 Release ${tag} 失败`);
  }
}

function buildMac() {
  step("本机打 Mac 包（Apple 芯片）");
  // 清掉上次的产物，免得打包没生成新文件时把旧包传上去。
  fs.rmSync(path.join(BUNDLE, "macos/GenDesk.app.tar.gz"), { force: true });
  fs.rmSync(path.join(BUNDLE, "macos/GenDesk.app.tar.gz.sig"), { force: true });
  for (const f of fs.existsSync(path.join(BUNDLE, "dmg"))
    ? fs.readdirSync(path.join(BUNDLE, "dmg"))
    : []) {
    if (f.endsWith(".dmg")) fs.rmSync(path.join(BUNDLE, "dmg", f), { force: true });
  }
  run("pnpm", ["tauri", "build", "--bundles", "app,dmg"], {
    env: {
      TAURI_SIGNING_PRIVATE_KEY: fs.readFileSync(SIGNING_KEY, "utf8"),
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "",
      APPLE_SIGNING_IDENTITY: SIGNING_IDENTITY,
    },
  });
}

/** 打包产物 → Release 上的文件名（与以前 tauri-action 传的一致，旧客户端的更新地址不变）。 */
function macAssets(version) {
  const files = {
    dmg: path.join(BUNDLE, `dmg/GenDesk_${version}_aarch64.dmg`),
    archive: path.join(BUNDLE, "macos/GenDesk.app.tar.gz"),
    sig: path.join(BUNDLE, "macos/GenDesk.app.tar.gz.sig"),
  };
  for (const f of Object.values(files)) if (!fs.existsSync(f)) fail(`打包后没找到 ${f}`);
  const staged = fs.mkdtempSync(path.join(os.tmpdir(), "gendesk-release-"));
  const names = {
    dmg: `GenDesk_${version}_aarch64.dmg`,
    archive: "GenDesk_aarch64.app.tar.gz",
    sig: "GenDesk_aarch64.app.tar.gz.sig",
  };
  const out = {};
  for (const [k, name] of Object.entries(names)) {
    out[k] = path.join(staged, name);
    fs.copyFileSync(files[k], out[k]);
  }
  return out;
}

async function main() {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: { "dry-run": { type: "boolean", default: false } },
  });
  const version = positionals[0];
  if (!version) fail("用法：pnpm release <X.Y.Z> [--dry-run]");
  const dryRun = values["dry-run"];
  const tag = `v${version}`;
  const started = Date.now();

  checkPreconditions(version, dryRun);
  if (!dryRun) {
    bumpAndTag(version);
    ensureRelease(tag);
  }
  buildMac();
  const assets = macAssets(dryRun ? currentVersion() : version);
  if (dryRun) {
    say(`\n试发完成，Mac 包在：\n${Object.values(assets).join("\n")}`);
    return;
  }

  step("上传 Mac 包并写进 latest.json");
  run("gh", [
    "release",
    "upload",
    tag,
    "--repo",
    REPO,
    "--clobber",
    assets.dmg,
    assets.archive,
    assets.sig,
  ]);
  await add({ tag, platform: "darwin", asset: path.basename(assets.archive), sigFile: assets.sig });

  step("更新本机 gendesk 命令");
  run("node", ["scripts/install-gendesk.mjs"], { capture: true });

  const minutes = ((Date.now() - started) / 60_000).toFixed(1);
  say(`\n✓ Mac 版 ${tag} 已发布（${minutes} 分钟），Mac 电脑现在就能收到更新。`);
  say(
    `  Windows 版在 GitHub 上打包，完成后自动补进 latest.json：https://github.com/${REPO}/actions/workflows/release.yml`,
  );
}

main().catch((e) => fail(e.message));
