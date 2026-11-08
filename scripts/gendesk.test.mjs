import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const exec = promisify(execFile);
const cli = fileURLToPath(new URL("./gendesk.mjs", import.meta.url));
// Use a fully decodable PNG now that validate and submit invoke Rust image decoding.
const png = await fs.readFile(new URL("../src-tauri/icons/32x32.png", import.meta.url));

async function fixture(t) {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "gendesk-cli-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const root = path.join(home, "交接目录");
  const input = path.join(home, "正文与参考图");
  await fs.mkdir(path.join(home, "GenDesk交接"));
  await fs.writeFile(
    path.join(home, "GenDesk交接", "连接.json"),
    JSON.stringify({
      version: 1,
      root,
      enabled: true,
      features: [
        "ordered-references",
        "generation-quality",
        "intake-preflight",
        "resumable-intake",
      ],
    }),
  );
  await fs.mkdir(input);
  await fs.writeFile(path.join(input, "桌面场景.txt"), "比例：3:4 的竖构图\n保留商品外观。\n");
  await fs.writeFile(path.join(input, "商品实拍.png"), png);
  const args = [
    "--prompt",
    path.join(input, "桌面场景.txt"),
    "--ref",
    path.join(input, "商品实拍.png"),
    "--sku",
    "55",
    "--style",
    "台历桌面",
    "--ratio",
    "3:4",
  ];
  // CLI orchestration tests use a protocol double. Real Rust validation is tested by
  // cargo tests and by GENDESK_TEST_APP_BIN in the native integration pass.
  const mock = path.join(home, "预检程序.mjs");
  await fs.writeFile(
    mock,
    `#!/usr/bin/env node
import fs from "node:fs";
const input = JSON.parse(fs.readFileSync(0, "utf8"));
if (process.argv[2] !== "validate-intake") process.exit(2);
process.stdout.write(JSON.stringify({status:"validated",validation:"full",protocolVersion:1,
  taskCount:input.order.items.length,refCount:Object.keys(input.references).length}));
`,
  );
  await fs.chmod(mock, 0o755);
  async function runWithNative(appBin, ...values) {
    const options = {
      env: {
        ...process.env,
        HOME: home,
        USERPROFILE: home,
        GENDESK_ROOT: root,
        GENDESK_APP_BIN: appBin,
      },
      timeout: 10000,
    };
    try {
      const result = await exec(process.execPath, [cli, ...values], options);
      return { code: 0, data: JSON.parse(result.stdout), stderr: result.stderr };
    } catch (e) {
      return { code: e.code, data: JSON.parse(e.stdout), stderr: e.stderr };
    }
  }
  const run = (...values) => runWithNative(process.env.GENDESK_TEST_APP_BIN ?? mock, ...values);
  return { home, root, input, args, run, runWithNative, mock };
}

test("single submit preserves body, copies inputs and deduplicates retries, including archived orders", async (t) => {
  const f = await fixture(t);
  const first = await f.run("submit", ...f.args);
  assert.equal(first.code, 0);
  assert.equal(first.data.status, "queued");
  const dir = first.data.dir;
  const order = JSON.parse(await fs.readFile(path.join(dir, "工单.json"), "utf8"));
  assert.equal(order.version, 1);
  assert.deepEqual(order.defaults, { quality: "high", ratio: "3:4" });
  assert.equal(order.items[0].name, "桌面场景");
  assert.equal(order.items[0].sku, "55");
  assert.equal(order.items[0].style, "台历桌面");
  assert.equal(
    await fs.readFile(path.join(dir, "prompts", "桌面场景.txt"), "utf8"),
    "比例：3:4 的竖构图\n保留商品外观。\n",
  );
  assert.deepEqual(await fs.readFile(path.join(dir, order.items[0].ref)), png);
  assert.equal((await fs.lstat(path.join(dir, order.items[0].ref))).isSymbolicLink(), false);
  assert.equal(await fs.readFile(path.join(dir, "READY.txt"), "utf8"), "ready\n");
  const again = await f.run("submit", ...f.args);
  assert.equal(again.data.jobId, first.data.jobId);
  assert.equal(again.data.reused, true);
  assert.deepEqual(await fs.readdir(path.dirname(dir)), [first.data.jobId]);

  const archived = path.join(f.root, "生图", "_已收录", `${first.data.jobId}-123`);
  await fs.mkdir(path.dirname(archived), { recursive: true });
  await fs.rename(dir, archived);
  await fs.writeFile(
    path.join(archived, "回执.json"),
    JSON.stringify({
      version: 1,
      job: { jobId: first.data.jobId, status: "done", batchIds: [42], taskCount: 1 },
    }),
  );
  const afterArchive = await f.run("submit", ...f.args);
  assert.equal(afterArchive.data.status, "ingested");
  assert.equal(afterArchive.data.reused, true);
  assert.deepEqual(afterArchive.data.receipt.batchIds, [42]);
  assert.deepEqual(await fs.readdir(path.dirname(dir)), []);
});

test("batch paths resolve relative to manifest; different image contents with the same basename stay paired", async (t) => {
  const f = await fixture(t);
  await fs.mkdir(path.join(f.input, "另一款"));
  await fs.writeFile(
    path.join(f.input, "另一款", "商品实拍.png"),
    Buffer.concat([png, Buffer.from("second")]),
  );
  const manifest = path.join(f.input, "场景任务.json");
  await fs.writeFile(
    manifest,
    JSON.stringify({
      defaults: { ratio: "3:4", format: "jpg" },
      items: [
        {
          name: "台历正面",
          sku: "55",
          style: "桌面场景",
          promptFile: "桌面场景.txt",
          ref: "商品实拍.png",
        },
        {
          name: "台历侧面",
          sku: "56",
          style: "桌面场景",
          promptFile: "桌面场景.txt",
          ref: "另一款/商品实拍.png",
          ratio: "9:16",
        },
        {
          name: "台历远景",
          sku: "55",
          style: "桌面场景",
          promptFile: "桌面场景.txt",
          ref: "商品实拍.png",
        },
      ],
    }),
  );
  const result = await f.run("submit", "--manifest", manifest);
  assert.equal(result.code, 0);
  assert.equal(result.data.taskCount, 3);
  assert.equal(result.data.refCount, 2);
  const order = JSON.parse(await fs.readFile(path.join(result.data.dir, "工单.json"), "utf8"));
  assert.notEqual(order.items[0].ref, order.items[1].ref);
  assert.equal(order.items[0].ref, order.items[2].ref);
  assert.equal(order.items[1].ratio, "9:16");
  assert.equal(order.defaults.format, "jpg");
  assert.equal(
    await fs.readFile(path.join(result.data.dir, "prompts", "台历侧面.txt"), "utf8"),
    "比例：3:4 的竖构图\n保留商品外观。\n",
  );
});

test("preflight never creates the inbox; missing or invalid files fail before READY", async (t) => {
  const f = await fixture(t);
  const valid = await f.run("validate", ...f.args);
  assert.equal(valid.code, 0);
  // validate now guarantees native parameter/decode preflight, rather than files-only.
  assert.equal(valid.data.validation, "full");
  assert.equal(valid.data.preflight.validation, "full");
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
  await fs.writeFile(path.join(f.input, "桌面场景.txt"), Buffer.from([0xff, 0xfe, 0xff]));
  const badEncoding = await f.run("submit", ...f.args);
  assert.equal(badEncoding.code, 2);
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
  await fs.rm(path.join(f.input, "商品实拍.png"));
  assert.equal((await f.run("submit", ...f.args)).code, 2);
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
});

test("ambiguous names, unknown fields and mixed CLI/manifest arguments fail before submission", async (t) => {
  const f = await fixture(t);
  const manifest = path.join(f.input, "错误清单.json");
  const item = {
    name: "桌面场景",
    sku: "55",
    style: "桌面",
    promptFile: "桌面场景.txt",
    ref: "商品实拍.png",
  };
  for (const items of [
    [item, item],
    [{ ...item, name: "../逃逸" }],
    [{ ...item, promptPath: "拼错字段" }],
    [{ ...item, sku: 55 }],
  ]) {
    await fs.writeFile(manifest, JSON.stringify({ items }));
    assert.equal((await f.run("submit", "--manifest", manifest)).code, 2);
  }
  assert.equal((await f.run("submit", "--manifest", manifest, "--ratio", "3:4")).code, 2);
  assert.equal((await f.run("doctor", "--open")).code, 2);
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
});

test("explicit ID cannot overwrite different content; an intentional fresh ID submits a new order", async (t) => {
  const f = await fixture(t);
  const first = await f.run("submit", ...f.args, "--job-id", "固定工单");
  await fs.writeFile(path.join(f.input, "桌面场景.txt"), "另一条词");
  const collision = await f.run("submit", ...f.args, "--job-id", "固定工单");
  assert.equal(collision.code, 2);
  assert.equal(
    await fs.readFile(path.join(first.data.dir, "prompts", "桌面场景.txt"), "utf8"),
    "比例：3:4 的竖构图\n保留商品外观。\n",
  );
  assert.equal((await f.run("submit", ...f.args, "--job-id", "另一工单")).code, 0);
});

test("status reads new and legacy receipts with useful exit codes; wait timeout leaves the queued job alone", async (t) => {
  const f = await fixture(t);
  const submitted = await f.run("submit", ...f.args, "--wait", "0.01");
  assert.equal(submitted.code, 3);
  assert.equal(submitted.data.timedOut, true);
  const { dir, jobId } = submitted.data;
  assert.equal((await f.run("status", "不存在")).code, 7);
  for (const [file, state, code] of [
    ["待确认.txt", "hold", 4],
    ["错误.txt", "error", 5],
    ["重名未收录.txt", "duplicate", 6],
    ["结果.txt", "ingested", 0],
  ]) {
    await fs.writeFile(path.join(dir, file), `legacy ${state}`);
    const result = await f.run("status", jobId);
    assert.equal(result.code, code);
    assert.equal(result.data.status, state);
    await fs.rm(path.join(dir, file));
  }
  await fs.writeFile(
    path.join(dir, "回执.json"),
    JSON.stringify({ version: 1, job: { jobId, status: "running" } }),
  );
  assert.equal((await f.run("status", jobId)).data.status, "ingesting");
  assert.equal((await f.run("status", jobId, "--wait", "0.01")).code, 3);
});

test("connection discovery respects custom root, explicit root and disabled intake without writing a job", async (t) => {
  const f = await fixture(t);
  const defaultRoot = path.join(f.home, "GenDesk交接");
  await fs.mkdir(defaultRoot, { recursive: true });
  await fs.writeFile(
    path.join(defaultRoot, "连接.json"),
    JSON.stringify({
      version: 1,
      root: f.root,
      enabled: false,
      taskThreshold: 20,
      appVersion: "0.34.1",
    }),
  );
  const doctor = await f.run("doctor");
  assert.equal(doctor.data.enabled, false);
  assert.equal(doctor.data.taskThreshold, 20);
  assert.equal((await f.run("submit", ...f.args)).code, 8);
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
  const explicit = await f.run("doctor", "--root", path.join(f.home, "别的目录"));
  assert.equal(explicit.data.source, "--root");
  assert.equal(explicit.data.enabled, null);
});

test("two simultaneous clients never publish two jobs for the same content", async (t) => {
  const f = await fixture(t);
  const results = await Promise.all([f.run("submit", ...f.args), f.run("submit", ...f.args)]);
  assert(results.some((result) => result.code === 0));
  const pending = path.join(f.root, "生图", "收件");
  const names = await fs.readdir(pending);
  assert.equal(names.length, 1);
  assert.equal(await fs.readFile(path.join(pending, names[0], "READY.txt"), "utf8"), "ready\n");
});

test("an unrelated malformed order cannot block other jobs; internal directory IDs are refused", async (t) => {
  const f = await fixture(t);
  const broken = path.join(f.root, "生图", "收件", "zzz-broken-order");
  await fs.mkdir(broken, { recursive: true });
  await fs.writeFile(path.join(broken, "工单.json"), "{incomplete");
  const result = await f.run("submit", ...f.args);
  assert.equal(result.code, 0);
  assert.equal((await f.run("status", result.data.jobId)).data.status, "queued");
  assert.equal((await f.run("submit", ...f.args, "--job-id", "_内部目录")).code, 2);
  await assert.rejects(fs.stat(path.join(f.root, "生图", "收件", "_内部目录")), { code: "ENOENT" });
  await fs.rm(path.join(result.data.dir, "READY.txt"));
  const incomplete = await f.run("status", result.data.jobId);
  assert.equal(incomplete.code, 2);
  assert.equal(incomplete.data.status, "staging");
});

test("multi-reference orders preserve Image 1/Image 2 order and require an app that supports attachments", async (t) => {
  const f = await fixture(t);
  await fs.rm(path.join(f.home, "GenDesk交接", "连接.json"));
  await fs.writeFile(path.join(f.input, "桌面参考.png"), Buffer.concat([png, Buffer.from("desk")]));
  const manifest = path.join(f.input, "双图清单.json");
  const item = {
    name: "日历桌面",
    sku: "日历款式",
    style: "桌面",
    promptFile: "桌面场景.txt",
    refs: ["商品实拍.png", "桌面参考.png"],
  };
  await fs.writeFile(manifest, JSON.stringify({ items: [item] }));
  assert.equal((await f.run("validate", "--manifest", manifest)).code, 0);
  assert.equal((await f.run("submit", "--manifest", manifest)).code, 2);
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
  const discovery = path.join(f.home, "GenDesk交接");
  await fs.mkdir(discovery, { recursive: true });
  await fs.writeFile(
    path.join(discovery, "连接.json"),
    JSON.stringify({
      version: 1,
      root: f.root,
      enabled: true,
      features: [
        "ordered-references",
        "generation-quality",
        "intake-preflight",
        "resumable-intake",
      ],
    }),
  );
  const result = await f.run("submit", "--manifest", manifest);
  assert.equal(result.code, 0);
  assert.equal(result.data.refCount, 2);
  const order = JSON.parse(await fs.readFile(path.join(result.data.dir, "工单.json"), "utf8"));
  assert.equal(order.items[0].refs.length, 2);
  assert.deepEqual(await fs.readFile(path.join(result.data.dir, order.items[0].refs[0])), png);
  assert.deepEqual(
    await fs.readFile(path.join(result.data.dir, order.items[0].refs[1])),
    Buffer.concat([png, Buffer.from("desk")]),
  );
  await fs.writeFile(manifest, JSON.stringify({ items: [{ ...item, ref: "商品实拍.png" }] }));
  assert.equal((await f.run("validate", "--manifest", manifest)).code, 2);
});

test("quality defaults to high; CLI, batch defaults and item overrides survive submission", async (t) => {
  const f = await fixture(t);
  const single = await f.run("submit", ...f.args, "--quality", " MAX ");
  assert.equal(single.code, 0);
  const order = JSON.parse(await fs.readFile(path.join(single.data.dir, "工单.json"), "utf8"));
  assert.equal(order.defaults.quality, "max");
  const item = {
    name: "默认质量",
    sku: "55",
    style: "桌面",
    promptFile: "桌面场景.txt",
    ref: "商品实拍.png",
  };
  const manifest = path.join(f.input, "质量清单.json");
  await fs.writeFile(
    manifest,
    JSON.stringify({
      defaults: { quality: "medium" },
      items: [item, { ...item, name: "单项质量", quality: "high" }],
    }),
  );
  const batch = await f.run("submit", "--manifest", manifest);
  assert.equal(batch.code, 0);
  const batchOrder = JSON.parse(await fs.readFile(path.join(batch.data.dir, "工单.json"), "utf8"));
  assert.equal(batchOrder.defaults.quality, "medium");
  assert.equal(batchOrder.items[1].quality, "high");
  for (const quality of ["hgh", "", 1, null]) {
    await fs.writeFile(manifest, JSON.stringify({ defaults: { quality }, items: [item] }));
    assert.equal((await f.run("validate", "--manifest", manifest)).code, 2);
  }
  assert.equal((await f.run("validate", ...f.args, "--quality", "hgh")).code, 2);
});

test("an older running app cannot silently swallow the new quality parameter", async (t) => {
  const f = await fixture(t);
  await fs.writeFile(
    path.join(f.home, "GenDesk交接", "连接.json"),
    JSON.stringify({ version: 1, root: f.root, enabled: true, features: ["ordered-references"] }),
  );
  const result = await f.run("submit", ...f.args);
  assert.equal(result.code, 2);
  assert.match(result.data.message, /quality/);
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
});

test("explicit content ownership is retained and unsupported apps cannot silently discard it", async (t) => {
  const f = await fixture(t);
  const rejected = await f.run(
    "submit",
    ...f.args,
    "--product",
    "陶瓷水杯",
    "--requirement",
    "厨房场景图",
  );
  assert.equal(rejected.code, 2);
  const discovery = path.join(f.home, "GenDesk交接", "连接.json");
  const conn = JSON.parse(await fs.readFile(discovery, "utf8"));
  conn.features.push("content-organization");
  await fs.writeFile(discovery, JSON.stringify(conn));
  const result = await f.run(
    "submit",
    ...f.args,
    "--product",
    "陶瓷水杯",
    "--requirement",
    "厨房场景图",
    "--source",
    "任务单",
  );
  assert.equal(result.code, 0);
  const order = JSON.parse(await fs.readFile(path.join(result.data.dir, "工单.json"), "utf8"));
  assert.deepEqual(order.organization, {
    product: "陶瓷水杯",
    style: "台历桌面",
    requirement: "厨房场景图",
    source: "任务单",
  });
});

test("version works without a connection and matches the application release", async (t) => {
  const f = await fixture(t);
  await fs.writeFile(path.join(f.home, "GenDesk交接", "连接.json"), "broken snapshot");
  const result = await f.run("--version");
  const pkg = JSON.parse(await fs.readFile(new URL("../package.json", import.meta.url), "utf8"));
  const app = JSON.parse(
    await fs.readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
  );
  assert.equal(result.code, 0);
  assert.equal(result.data.cliVersion, pkg.version);
  assert.equal(result.data.cliVersion, app.version);
  assert.equal(result.data.orderVersion, 1);
  assert.ok(result.data.clientFeatures.includes("content-organization"));
  assert.equal((await f.run("doctor", "--version")).code, 2);
});

test("doctor distinguishes client support from the application discovery snapshot", async (t) => {
  const f = await fixture(t);
  const result = await f.run("doctor");
  assert.equal(result.code, 0);
  assert.ok(result.data.cliVersion);
  assert.ok(result.data.clientFeatures.includes("content-organization"));
  assert.ok(!result.data.features.includes("content-organization"));
  assert.match(result.data.message, /不代表应用在线/);
});

test("organization permits non-product tasks and item ownership replaces the entire default", async (t) => {
  const f = await fixture(t);
  const discovery = path.join(f.home, "GenDesk交接", "连接.json");
  const conn = JSON.parse(await fs.readFile(discovery, "utf8"));
  conn.features.push("content-organization");
  await fs.writeFile(discovery, JSON.stringify(conn));
  const manifest = path.join(f.input, "归属清单.json");
  const organization = {
    product: "陶瓷水杯",
    style: "奶油白款",
    requirement: "厨房场景图",
    source: "任务单",
  };
  const item = { name: "厨房晨光", promptFile: "桌面场景.txt", ref: "商品实拍.png" };
  const spec = {
    version: 1,
    organization,
    items: [item, { ...item, name: "空间氛围", organization: { requirement: "空间概念图" } }],
  };
  await fs.writeFile(manifest, JSON.stringify(spec));
  const result = await f.run("submit", "--manifest", manifest);
  assert.equal(result.code, 0);
  const order = JSON.parse(await fs.readFile(path.join(result.data.dir, "工单.json"), "utf8"));
  assert.deepEqual(order.organization, organization);
  assert.equal(order.items[0].sku, "");
  assert.equal(order.items[0].style, "奶油白款");
  assert.deepEqual(order.items[1].organization, { requirement: "空间概念图" });
  assert.equal(order.items[1].style, "");
  spec.organization.product = " 陶瓷水杯";
  await fs.writeFile(manifest, JSON.stringify(spec));
  assert.equal((await f.run("validate", "--manifest", manifest)).code, 2);
});

test("installer replaces a standalone command without changing discovery or creating orders", async (t) => {
  if (process.platform === "win32") return t.skip("Unix command installer");
  const f = await fixture(t);
  const binDir = path.join(f.home, "命令入口");
  await fs.mkdir(binDir);
  const installed = path.join(binDir, "gendesk");
  await fs.writeFile(installed, "old CLI");
  const discovery = path.join(f.home, "GenDesk交接", "连接.json");
  const before = await fs.readFile(discovery);
  const { stdout } = await exec(
    process.execPath,
    [fileURLToPath(new URL("./install-gendesk.mjs", import.meta.url)), "--bin-dir", binDir],
    { env: { ...process.env, HOME: f.home }, timeout: 10000 },
  );
  const result = JSON.parse(stdout);
  assert.equal(result.status, "installed");
  assert.equal(result.path, installed);
  // The executable is a module-neutral launcher; the explicit .mjs payload preserves the source.
  assert.equal(result.contentPath, path.join(binDir, "gendesk.mjs"));
  assert.deepEqual(await fs.readFile(result.contentPath), await fs.readFile(cli));
  assert.ok((await fs.stat(installed)).mode & 0o111);
  assert.deepEqual(await fs.readdir(binDir), ["gendesk", "gendesk.mjs"]);
  const version = JSON.parse((await exec(installed, ["--version"])).stdout);
  assert.equal(version.cliVersion, result.cliVersion);
  assert.deepEqual(await fs.readFile(discovery), before);
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
});

test("installed command works without module detection in CommonJS and ESM directories", async (t) => {
  if (process.platform === "win32") return t.skip("Unix command installer");
  const f = await fixture(t);
  for (const type of ["commonjs", "module"]) {
    const binDir = path.join(f.home, type);
    await fs.mkdir(binDir);
    await fs.writeFile(path.join(binDir, "package.json"), JSON.stringify({ type }));
    const installer = fileURLToPath(new URL("./install-gendesk.mjs", import.meta.url));
    const { stdout } = await exec(process.execPath, [installer, "--bin-dir", binDir]);
    const installed = JSON.parse(stdout);
    const result = await exec(process.execPath, [
      "--no-experimental-detect-module",
      installed.path,
      "--version",
    ]);
    assert.equal(JSON.parse(result.stdout).cliVersion, installed.cliVersion);
  }
});

test("installer refuses mismatched release versions and preserves the previous command", async (t) => {
  if (process.platform === "win32") return t.skip("Unix command installer");
  const f = await fixture(t);
  const release = path.join(f.home, "发布副本");
  await fs.mkdir(path.join(release, "scripts"), { recursive: true });
  await fs.mkdir(path.join(release, "src-tauri"));
  const installer = path.join(release, "scripts", "install-gendesk.mjs");
  await fs.copyFile(new URL("./install-gendesk.mjs", import.meta.url), installer);
  await fs.copyFile(cli, path.join(release, "scripts", "gendesk.mjs"));
  // Both release configs claim a new version while the bundled client is still old.
  await fs.writeFile(path.join(release, "package.json"), JSON.stringify({ version: "999.0.0" }));
  await fs.writeFile(
    path.join(release, "src-tauri", "tauri.conf.json"),
    JSON.stringify({ version: "999.0.0" }),
  );
  const binDir = path.join(f.home, "命令入口");
  await fs.mkdir(binDir);
  const installed = path.join(binDir, "gendesk");
  await fs.writeFile(installed, "previous CLI");
  await assert.rejects(exec(process.execPath, [installer, "--bin-dir", binDir]), (error) => {
    assert.equal(error.code, 2);
    assert.match(error.stderr, /不一致/);
    return true;
  });
  assert.equal(await fs.readFile(installed, "utf8"), "previous CLI");
  assert.deepEqual(await fs.readdir(binDir), ["gendesk"]);
});

test("the canonical documentation manifest is accepted by the current CLI", async (t) => {
  const f = await fixture(t);
  const docs = await fs.readFile(new URL("../docs/命令行生图接入.md", import.meta.url), "utf8");
  const examples = [...docs.matchAll(/```json\n([\s\S]*?)\n```/g)];
  assert.ok(examples.length > 0);
  for (const [index, match] of examples.entries()) {
    const spec = JSON.parse(match[1]);
    for (const item of spec.items) {
      const prompt = path.join(f.input, item.promptFile);
      await fs.mkdir(path.dirname(prompt), { recursive: true });
      await fs.writeFile(prompt, "Image 1 为产品，Image 2 为环境。保留产品结构。\n");
      for (const input of item.refs ?? [item.ref]) {
        const image = path.join(f.input, input);
        await fs.mkdir(path.dirname(image), { recursive: true });
        await fs.writeFile(image, Buffer.concat([png, Buffer.from(input)]));
      }
    }
    const manifest = path.join(f.input, `文档清单${index}.json`);
    await fs.writeFile(manifest, JSON.stringify(spec));
    const result = await f.run("validate", "--manifest", manifest);
    assert.equal(result.code, 0, result.data.message);
    assert.equal(result.data.taskCount, spec.items.length);
  }
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
});

test("Rust preflight errors and malformed responses never publish READY or an inbox", async (t) => {
  const f = await fixture(t);
  for (const script of [
    'process.stdout.write(JSON.stringify({message:"图片解码失败"})); process.exitCode=2;',
    'process.stdout.write("not json");',
    'process.stdout.write(JSON.stringify({protocolVersion:1,validation:"full",taskCount:999}));',
  ]) {
    await fs.writeFile(f.mock, `#!/usr/bin/env node\n${script}\n`);
    const result = await f.runWithNative(f.mock, "submit", ...f.args);
    assert.equal(result.code, 2);
    await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
  }
});

test("a consumer without durable intake support is refused before any submission", async (t) => {
  const f = await fixture(t);
  const discovery = path.join(f.home, "GenDesk交接", "连接.json");
  const conn = JSON.parse(await fs.readFile(discovery, "utf8"));
  conn.features = ["ordered-references", "generation-quality", "intake-preflight"];
  await fs.writeFile(discovery, JSON.stringify(conn));
  const result = await f.run("submit", ...f.args);
  assert.equal(result.code, 2);
  assert.match(result.data.message, /安全恢复/);
  await assert.rejects(fs.stat(f.root), { code: "ENOENT" });
});
