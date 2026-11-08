import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const exec = promisify(execFile);
const cli = fileURLToPath(new URL("./gendesk.mjs", import.meta.url));
const png = await fs.readFile(new URL("../src-tauri/icons/32x32.png", import.meta.url));

/**
 * A temp HOME with prompt/ref inputs, a validate-intake double and (optionally) a fake
 * GenDesk control channel. Real Rust ingestion is covered by cargo tests; this file
 * checks the client's packaging, protocol and exit codes.
 */
async function fixture(t, { online = true, hold = false } = {}) {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "gendesk-cli-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const root = path.join(home, "GenDesk交接");
  const input = path.join(home, "正文与参考图");
  await fs.mkdir(root);
  await fs.mkdir(input);
  await fs.writeFile(path.join(input, "桌面场景.txt"), "比例：3:4 的竖构图\n保留商品外观。\n");
  await fs.writeFile(path.join(input, "商品实拍.png"), png);
  const args = [
    "--prompt",
    path.join(input, "桌面场景.txt"),
    "--ref",
    path.join(input, "商品实拍.png"),
    "--product",
    "陶瓷水杯",
    "--style",
    "奶油白款",
    "--requirement",
    "厨房场景图",
    "--ratio",
    "3:4",
  ];
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

  const calls = [];
  const jobs = new Map();
  const pending = path.join(root, "生图", "收件");
  const consumed = path.join(root, "生图", "_已收录");
  const handlers = {
    hello: () => ({ protocol: 2, root, taskThreshold: 100 }),
    "intake.status": ({ jobId }) => ({ job: jobs.get(jobId) ?? null }),
    "intake.submit": async ({ dir }) => {
      const order = JSON.parse(await fs.readFile(path.join(pending, dir, "工单.json"), "utf8"));
      await fs.stat(path.join(pending, dir, "READY.txt"));
      const job = {
        jobId: order.jobId,
        dirName: dir,
        status: hold ? "hold" : "done",
        taskCount: order.items.length,
        message: "",
      };
      jobs.set(order.jobId, job);
      if (!hold) {
        await fs.mkdir(consumed, { recursive: true });
        await fs.rename(path.join(pending, dir), path.join(consumed, `${dir}-1700000000`));
      }
      return { job };
    },
    "intake.confirm": ({ jobId }) => {
      const job = { ...jobs.get(jobId), status: "done" };
      jobs.set(jobId, job);
      return { job };
    },
    overview: () => ({ tasks: { queued: 1, running: 0, review: 2, failed: 0 }, paused: false }),
    "v2v.pending": () => ({ groups: [], skipped: 0 }),
    "v2v.rewrite": ({ lines }) => ({ applied: lines.length }),
  };
  const token = "test-token";
  let port = 1;
  if (online) {
    const server = net.createServer((socket) => {
      let body = "";
      socket.setEncoding("utf8");
      socket.on("data", async (chunk) => {
        body += chunk;
        if (!body.includes("\n")) return;
        const req = JSON.parse(body);
        calls.push(req);
        let reply;
        try {
          assert.equal(req.token, token);
          reply = { ok: true, data: await handlers[req.cmd](req.args) };
        } catch (e) {
          reply = { ok: false, error: e.message };
        }
        socket.end(`${JSON.stringify(reply)}\n`);
      });
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    t.after(() => server.close());
    port = server.address().port;
  }
  const connection = {
    version: 2,
    protocol: 2,
    port,
    token,
    appPath: mock,
    root,
    taskThreshold: 100,
  };
  await fs.writeFile(path.join(root, "连接.json"), JSON.stringify(connection));

  async function run(...values) {
    const options = {
      env: { ...process.env, HOME: home, USERPROFILE: home, GENDESK_LAUNCH: "0" },
      timeout: 15000,
    };
    try {
      const result = await exec(process.execPath, [cli, ...values], options);
      return { code: 0, data: JSON.parse(result.stdout) };
    } catch (e) {
      return { code: e.code, data: JSON.parse(e.stdout) };
    }
  }
  return { home, root, input, args, mock, calls, jobs, pending, consumed, run, connection };
}

const commands = (f) => f.calls.map((c) => c.cmd);

test("version works without a connection and matches the application release", async (t) => {
  const f = await fixture(t, { online: false });
  await fs.writeFile(path.join(f.root, "连接.json"), "broken");
  const result = await f.run("--version");
  const pkg = JSON.parse(await fs.readFile(new URL("../package.json", import.meta.url), "utf8"));
  const app = JSON.parse(
    await fs.readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
  );
  assert.equal(result.code, 0);
  assert.equal(result.data.cliVersion, pkg.version);
  assert.equal(result.data.cliVersion, app.version);
  assert.equal(result.data.protocol, 2);
  assert.equal((await f.run("doctor", "--version")).code, 2);
});

test("doctor reports online and offline without launching or writing anything", async (t) => {
  const online = await fixture(t);
  const up = await online.run("doctor");
  assert.equal(up.code, 0);
  assert.equal(up.data.status, "online");
  assert.equal(up.data.app.protocol, 2);
  const offline = await fixture(t, { online: false });
  const down = await offline.run("doctor");
  assert.equal(down.code, 0);
  assert.equal(down.data.status, "offline");
  await assert.rejects(fs.stat(offline.pending), { code: "ENOENT" });
});

test("submit packages the order, publishes READY last and ingests over the channel", async (t) => {
  const f = await fixture(t);
  const result = await f.run("submit", ...f.args);
  assert.equal(result.code, 0, result.data.message);
  assert.equal(result.data.status, "ingested");
  assert.match(result.data.jobId, /^cli-[a-f0-9]{32}$/);
  assert.deepEqual(commands(f), ["hello", "intake.status", "intake.submit"]);
  assert.equal(f.calls[2].args.dir, result.data.jobId);
  const [archived] = await fs.readdir(f.consumed);
  const dir = path.join(f.consumed, archived);
  const order = JSON.parse(await fs.readFile(path.join(dir, "工单.json"), "utf8"));
  assert.equal(order.version, 1);
  assert.equal(order.defaults.quality, "high");
  assert.equal(order.items[0].name, "桌面场景");
  assert.match(order.items[0].ref, /^refs\/[a-f0-9]{64}\.png$/);
  assert.deepEqual(order.organization, {
    product: "陶瓷水杯",
    style: "奶油白款",
    requirement: "厨房场景图",
    source: "CLI",
  });
  assert.ok(await fs.stat(path.join(dir, "READY.txt")));
});

test("resubmitting the same content reuses the job and never ingests twice", async (t) => {
  const f = await fixture(t);
  const first = await f.run("submit", ...f.args);
  const second = await f.run("submit", ...f.args);
  assert.equal(second.code, 0);
  assert.equal(second.data.reused, true);
  assert.equal(second.data.jobId, first.data.jobId);
  assert.equal(commands(f).filter((c) => c === "intake.submit").length, 1);
});

test("a reused jobId with different content is refused before anything is written", async (t) => {
  const f = await fixture(t);
  await f.run("submit", ...f.args, "--job-id", "固定编号");
  await fs.writeFile(path.join(f.input, "桌面场景.txt"), "换了内容\n");
  const result = await f.run("submit", ...f.args, "--job-id", "固定编号");
  assert.equal(result.code, 2);
  assert.match(result.data.message, /内容不同/);
  await assert.rejects(fs.stat(path.join(f.pending, "固定编号")), { code: "ENOENT" });
});

test("orders over the threshold stop at hold until confirm", async (t) => {
  const f = await fixture(t, { hold: true });
  const held = await f.run("submit", ...f.args);
  assert.equal(held.code, 4);
  assert.equal(held.data.status, "hold");
  assert.match(held.data.next, /gendesk confirm/);
  const confirmed = await f.run("confirm", held.data.jobId);
  assert.equal(confirmed.code, 0);
  assert.equal(confirmed.data.status, "ingested");
});

test("status of an unknown job exits 7", async (t) => {
  const f = await fixture(t);
  const result = await f.run("status", "不存在的单");
  assert.equal(result.code, 7);
  assert.equal(result.data.status, "not_found");
});

test("an offline app is reported with exit 8 and nothing is packaged", async (t) => {
  const f = await fixture(t, { online: false });
  const result = await f.run("submit", ...f.args);
  assert.equal(result.code, 8);
  await assert.rejects(fs.stat(f.pending), { code: "ENOENT" });
});

test("a connection file from an older protocol is refused", async (t) => {
  const f = await fixture(t);
  await fs.writeFile(
    path.join(f.root, "连接.json"),
    JSON.stringify({ version: 1, root: f.root, enabled: true }),
  );
  const result = await f.run("submit", ...f.args);
  assert.equal(result.code, 8);
  assert.match(result.data.message, /协议/);
});

test("preflight failures never publish a package", async (t) => {
  const f = await fixture(t);
  for (const script of [
    'process.stdout.write(JSON.stringify({message:"图片解码失败"})); process.exitCode=2;',
    'process.stdout.write("not json");',
    'process.stdout.write(JSON.stringify({protocolVersion:1,validation:"full",taskCount:999}));',
  ]) {
    await fs.writeFile(f.mock, `#!/usr/bin/env node\n${script}\n`);
    const result = await f.run("submit", ...f.args);
    assert.equal(result.code, 2);
    await assert.rejects(fs.stat(f.pending), { code: "ENOENT" });
  }
  assert.ok(!commands(f).includes("intake.submit"));
});

test("manifest validation rejects removed and conflicting fields", async (t) => {
  const f = await fixture(t);
  const manifest = path.join(f.input, "清单.json");
  const item = { name: "厨房晨光", promptFile: "桌面场景.txt", ref: "商品实拍.png" };
  const organization = { product: "陶瓷水杯", style: "奶油白款", requirement: "厨房场景图" };
  for (const spec of [
    { version: 1, organization, refGroup: "旧图库分组", items: [item] },
    { version: 1, organization, ephemeral: true, items: [item] },
    { version: 1, organization, items: [{ ...item, refs: ["商品实拍.png"] }] },
    { version: 2, organization, items: [item] },
  ]) {
    await fs.writeFile(manifest, JSON.stringify(spec));
    assert.equal((await f.run("validate", "--manifest", manifest)).code, 2);
  }
});

test("organization permits non-product tasks and item ownership replaces the default", async (t) => {
  const f = await fixture(t);
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
  const result = await f.run("validate", "--manifest", manifest);
  assert.equal(result.code, 0, result.data.message);
  const order = result.data.order;
  assert.deepEqual(order.organization, organization);
  assert.equal(order.items[0].sku, "");
  assert.equal(order.items[0].style, "奶油白款");
  assert.deepEqual(order.items[1].organization, { requirement: "空间概念图" });
  assert.equal(order.items[1].style, "");
  spec.organization.product = " 陶瓷水杯";
  await fs.writeFile(manifest, JSON.stringify(spec));
  assert.equal((await f.run("validate", "--manifest", manifest)).code, 2);
});

test("v2v rewrite forwards array, object and JSON Lines files", async (t) => {
  const f = await fixture(t);
  const file = path.join(f.input, "改写.json");
  const line = { id: "W12", videoPrompt: "镜头缓慢推近" };
  for (const body of [
    JSON.stringify([line]),
    JSON.stringify({ lines: [line, { ...line, id: "W13" }] }),
    `${JSON.stringify(line)}\n${JSON.stringify({ ...line, id: "W14" })}\n`,
  ]) {
    await fs.writeFile(file, body);
    const result = await f.run("v2v", "rewrite", "--file", file);
    assert.equal(result.code, 0, result.data.message);
    assert.ok(result.data.applied >= 1);
  }
  await fs.writeFile(file, JSON.stringify([{ videoPrompt: "没有编号" }]));
  assert.equal((await f.run("v2v", "rewrite", "--file", file)).code, 2);
  assert.equal((await f.run("v2v", "pending")).data.skipped, 0);
});

test("the canonical documentation manifest is accepted by the current CLI", async (t) => {
  const f = await fixture(t);
  const docs = await fs.readFile(new URL("../docs/命令行生图接入.md", import.meta.url), "utf8");
  const examples = [...docs.matchAll(/```json\n([\s\S]*?)\n```/g)].filter((m) =>
    m[1].includes('"items"'),
  );
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
  await assert.rejects(fs.stat(f.pending), { code: "ENOENT" });
});

test("installer replaces a standalone command without touching the connection", async (t) => {
  if (process.platform === "win32") return t.skip("Unix command installer");
  const f = await fixture(t, { online: false });
  const binDir = path.join(f.home, "命令入口");
  await fs.mkdir(binDir);
  const installed = path.join(binDir, "gendesk");
  await fs.writeFile(installed, "old CLI");
  const connection = path.join(f.root, "连接.json");
  const before = await fs.readFile(connection);
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
  assert.deepEqual(await fs.readFile(connection), before);
});

test("installed command works without module detection in CommonJS and ESM directories", async (t) => {
  if (process.platform === "win32") return t.skip("Unix command installer");
  const f = await fixture(t, { online: false });
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
  const f = await fixture(t, { online: false });
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
