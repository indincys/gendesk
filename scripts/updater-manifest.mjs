#!/usr/bin/env node
// 应用内更新清单 latest.json 的拼装（Mac 本机打包、Windows 在 GitHub 打包，两边并行）。
//
// 两边同时「下载 latest.json → 加上自己 → 覆盖上传」会互相覆盖（v0.2.2 就发出过只含一端的
// 清单）。所以每端只写自己的那一份 `updater-<平台>.json`，再把 Release 上现有的所有份拼成
// latest.json；后完成的那端必然看得到先完成那端的份。上传后再读一遍，缺了就重拼。
//
//   node scripts/updater-manifest.mjs add --tag v0.38.0 --platform darwin \
//        --asset GenDesk_aarch64.app.tar.gz --sig-file <…/GenDesk.app.tar.gz.sig>
//   node scripts/updater-manifest.mjs verify --tag v0.38.0 [--wait 40]
//
// 需要已登录的 gh（CI 里设 GH_TOKEN）。仓库默认取 GITHUB_REPOSITORY 或 indincys/gendesk。

import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const say = (text) => process.stdout.write(`${text}\n`);

export const NOTES = "详见 CHANGELOG。首次运行的 SmartScreen / Gatekeeper 放行指引见 README。";

/** 每个平台写进 latest.json 的键（与 tauri-action 生成的一致，新旧客户端都认）。 */
export const PLATFORM_KEYS = {
  darwin: ["darwin-aarch64", "darwin-aarch64-app"],
  windows: ["windows-x86_64", "windows-x86_64-nsis"],
};

/** 更新源必须覆盖的平台键（客户端只认这两个）。 */
export const REQUIRED = ["darwin-aarch64", "windows-x86_64"];

export function fragmentName(platform) {
  return `updater-${platform}.json`;
}

/** 一端的份：只含自己的平台条目。 */
export function fragment({ platform, url, signature }) {
  const keys = PLATFORM_KEYS[platform];
  if (!keys)
    throw new Error(`未知平台 ${platform}（可用：${Object.keys(PLATFORM_KEYS).join("、")}）`);
  if (!signature.trim()) throw new Error("签名为空");
  return {
    platforms: Object.fromEntries(keys.map((k) => [k, { signature: signature.trim(), url }])),
  };
}

/** 把所有份拼成 latest.json。 */
export function compose(tag, fragments, now = new Date()) {
  const platforms = {};
  for (const f of fragments) Object.assign(platforms, f.platforms);
  const sorted = Object.fromEntries(
    Object.keys(platforms)
      .sort()
      .map((k) => [k, platforms[k]]),
  );
  return {
    version: tag.replace(/^v/, ""),
    notes: NOTES,
    pub_date: now.toISOString(),
    platforms: sorted,
  };
}

/** latest.json 缺了哪些平台、版本对不对；空数组 = 完整。 */
export function problems(tag, manifest, required = REQUIRED) {
  const out = [];
  if (manifest?.version !== tag.replace(/^v/, ""))
    out.push(`version=${manifest?.version} 与 ${tag} 不一致`);
  for (const key of required) if (!manifest?.platforms?.[key]?.url) out.push(`缺少平台 ${key}`);
  return out;
}

// ---------------------------------------------------------------- gh

function repo() {
  return process.env.GITHUB_REPOSITORY || "indincys/gendesk";
}

function gh(args) {
  const r = spawnSync("gh", [...args, "--repo", repo()], { encoding: "utf8" });
  if (r.error || r.status !== 0) {
    throw new Error(`gh ${args.join(" ")} 失败：${r.error?.message ?? r.stderr.trim()}`);
  }
  return r.stdout;
}

function tmpdir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "gendesk-updater-"));
}

function assetNames(tag) {
  return JSON.parse(gh(["release", "view", tag, "--json", "assets"])).assets.map((a) => a.name);
}

function downloadJson(tag, name) {
  const dir = tmpdir();
  gh(["release", "download", tag, "-p", name, "-D", dir, "--clobber"]);
  return JSON.parse(fs.readFileSync(path.join(dir, name), "utf8"));
}

function uploadJson(tag, name, value) {
  const file = path.join(tmpdir(), name);
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
  gh(["release", "upload", tag, file, "--clobber"]);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 拼一次并上传；返回拼进去的平台键。 */
function rebuild(tag) {
  const names = assetNames(tag).filter((n) => /^updater-.+\.json$/.test(n));
  const manifest = compose(
    tag,
    names.map((n) => downloadJson(tag, n)),
  );
  uploadJson(tag, "latest.json", manifest);
  return Object.keys(manifest.platforms);
}

export async function add({ tag, platform, asset, sigFile }) {
  const url = `https://github.com/${repo()}/releases/download/${tag}/${asset}`;
  const signature = fs.readFileSync(sigFile, "utf8");
  uploadJson(tag, fragmentName(platform), fragment({ platform, url, signature }));
  // 另一端可能正在拼：拼完隔几秒再读，发现有份没进去就重拼。
  for (let round = 1; round <= 4; round += 1) {
    const keys = rebuild(tag);
    await sleep(5000);
    const expected = new Set(
      assetNames(tag)
        .filter((n) => /^updater-.+\.json$/.test(n))
        .flatMap((n) => PLATFORM_KEYS[n.slice("updater-".length, -".json".length)] ?? []),
    );
    const live = Object.keys(downloadJson(tag, "latest.json").platforms);
    if ([...expected].every((k) => live.includes(k))) {
      say(`latest.json 已含：${live.join("、")}`);
      return;
    }
    say(
      `第 ${round} 次拼装后缺 ${[...expected].filter((k) => !live.includes(k)).join("、")}（拼进 ${keys.join("、")}），重拼`,
    );
  }
  throw new Error("latest.json 多次拼装仍不完整，请重跑本命令");
}

/** 等到 latest.json 完整且每个平台的更新包都能下载。 */
export async function verify({ tag, waitMinutes }) {
  const deadline = Date.now() + waitMinutes * 60_000;
  for (;;) {
    let issues;
    try {
      const manifest = downloadJson(tag, "latest.json");
      issues = problems(tag, manifest);
      if (issues.length === 0) {
        for (const key of REQUIRED) {
          const res = await fetch(manifest.platforms[key].url, { redirect: "follow" });
          await res.body?.cancel();
          if (!res.ok) issues.push(`${key} 更新包不可下载（HTTP ${res.status}）`);
        }
      }
    } catch (e) {
      issues = [e.message];
    }
    if (issues.length === 0) {
      say(`✓ ${tag} 的 latest.json 平台齐全，更新包都能下载`);
      return;
    }
    if (Date.now() > deadline) throw new Error(`latest.json 不完整：${issues.join("；")}`);
    say(`等另一端：${issues.join("；")}`);
    await sleep(30_000);
  }
}

async function main() {
  const [command, ...rest] = process.argv.slice(2);
  const { values } = parseArgs({
    args: rest,
    options: {
      tag: { type: "string" },
      platform: { type: "string" },
      asset: { type: "string" },
      "sig-file": { type: "string" },
      wait: { type: "string", default: "0" },
    },
  });
  if (!values.tag) throw new Error("缺少 --tag");
  if (command === "add") {
    if (!values.platform || !values.asset || !values["sig-file"]) {
      throw new Error("add 需要 --platform --asset --sig-file");
    }
    await add({
      tag: values.tag,
      platform: values.platform,
      asset: values.asset,
      sigFile: values["sig-file"],
    });
  } else if (command === "verify") {
    await verify({ tag: values.tag, waitMinutes: Number(values.wait) });
  } else {
    throw new Error("用法：updater-manifest.mjs add|verify --tag vX.Y.Z …");
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch((e) => {
    console.error(`✗ ${e.message}`);
    process.exit(1);
  });
}
