#!/usr/bin/env node
// Install an explicit ESM payload and a module-neutral command launcher.
import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import * as fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

async function main() {
  const { values } = parseArgs({ options: { "bin-dir": { type: "string" } } });
  if (process.platform === "win32") {
    throw new Error("此安装入口用于 macOS/Unix；Windows 请用 node scripts/gendesk.mjs 调用");
  }
  const repo = fileURLToPath(new URL("../", import.meta.url));
  const pkg = JSON.parse(await fs.readFile(path.join(repo, "package.json"), "utf8"));
  const app = JSON.parse(await fs.readFile(path.join(repo, "src-tauri/tauri.conf.json"), "utf8"));
  if (pkg.version !== app.version) throw new Error("package.json 与应用版本不一致，请先同步版本");
  const bytes = await fs.readFile(new URL("./gendesk.mjs", import.meta.url));
  const binDir = values["bin-dir"] ?? path.join(os.homedir(), ".local", "bin");
  if (!binDir.trim()) throw new Error("--bin-dir 不能为空");
  const target = path.resolve(binDir, "gendesk");
  const contentPath = path.join(path.dirname(target), "gendesk.mjs");
  await fs.mkdir(path.dirname(target), { recursive: true });
  const staging = path.join(path.dirname(target), `.gendesk-${randomUUID()}`);
  const stagedCommand = path.join(staging, "gendesk");
  const stagedContent = path.join(staging, "gendesk.mjs");
  try {
    await fs.mkdir(staging);
    await fs.writeFile(stagedContent, bytes, { flag: "wx" });
    // Dynamic import parses as both CommonJS and ESM, including early Node 22 releases.
    await fs.writeFile(
      stagedCommand,
      '#!/usr/bin/env node\nimport("./gendesk.mjs").catch((error) => { process.stderr.write(`${error.message}\\n`); process.exitCode = 2; });\n',
      { flag: "wx", mode: 0o755 },
    );
    await fs.chmod(stagedCommand, 0o755);
    const version = JSON.parse(
      execFileSync(
        process.execPath,
        ["--no-experimental-detect-module", stagedCommand, "--version"],
        {
          encoding: "utf8",
          timeout: 10000,
        },
      ),
    );
    if (version.cliVersion !== pkg.version) {
      throw new Error(`CLI ${version.cliVersion} 与应用 ${pkg.version} 不一致，请先同步版本`);
    }
    // Verify the final entry form first; publish payload before the launcher that imports it.
    await fs.rename(stagedContent, contentPath);
    await fs.rename(stagedCommand, target);
    process.stdout.write(
      `${JSON.stringify({ status: "installed", cliVersion: version.cliVersion, path: target, contentPath, sha256: createHash("sha256").update(bytes).digest("hex") }, null, 2)}\n`,
    );
  } finally {
    await fs.rm(staging, { recursive: true, force: true });
  }
}

main().catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 2;
});
