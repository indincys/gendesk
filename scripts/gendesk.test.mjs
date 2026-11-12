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
async function fixture(
  t,
  { online = true, hold = false, features = ["reference-library", "poster-link"] } = {},
) {
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
    hello: () => ({ protocol: 2, root, taskThreshold: 100, features }),
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
      const libraryRefs = order.items.flatMap((item) =>
        (item.refs ?? [item.ref])
          .map((ref, slot) => ({ item: item.name, slot: slot + 1, ref }))
          .filter((x) => typeof x.ref === "object"),
      );
      if (!hold) {
        await fs.mkdir(consumed, { recursive: true });
        await fs.rename(path.join(pending, dir), path.join(consumed, `${dir}-1700000000`));
      }
      if (hold || !libraryRefs.length) return { job };
      return {
        job,
        libraryPicks: libraryRefs.map((x, i) => ({
          itemName: x.item,
          slot: x.slot,
          fallback: false,
          image: {
            id: 100 + i,
            style: "王一博",
            category: x.ref.category,
            name: "图",
            filePath: "/库/图.png",
          },
        })),
      };
    },
    "library.check": ({ slots }) => ({
      slots: slots.map((s) => ({
        reference: `图库 ${s.style}/${s.category}`,
        ok: s.category !== "礼盒图",
        poolSize: s.category === "礼盒图" ? 0 : 5,
        freshCount: 3,
        message: s.category === "礼盒图" ? "没有启用的图片" : "5 张候选",
      })),
    }),
    "library.import": ({ paths }) => ({
      imported: paths.map((p, i) => ({
        id: i + 1,
        style: "王一博",
        category: "正面图",
        name: path.basename(p),
        filePath: p,
        thumbPath: p,
        enabled: true,
        useCount: 0,
      })),
      restored: [],
      duplicates: [],
      failed: [],
    }),
    "library.pick": ({ count, markUsed }) => ({
      style: "王一博",
      category: "正面图",
      poolSize: 4,
      freshCount: 4,
      markedUsed: markUsed,
      message: "全部来自冷却期外的图",
      picks: Array.from({ length: count }, (_, i) => ({
        tier: "fresh",
        image: {
          id: i + 1,
          style: "王一博",
          category: "正面图",
          name: "图",
          filePath: "/库/图.png",
          thumbPath: "/t",
          enabled: true,
          useCount: 0,
        },
      })),
    }),
    "library.update": ({ ids }) => ({ changed: ids.length }),
    "library.pack": ({ slots, markUsed, exportDir }) => {
      let n = 0;
      const picks = slots.flatMap((s) =>
        Array.from({ length: s.count ?? 1 }, () => {
          n += 1;
          return {
            slot: n,
            tier: s.category === "礼盒图" ? "cooling" : "fresh",
            cooldownDays: s.category === "礼盒图" ? 3 : 7,
            configuredCooldownDays: 7,
            exportedFile: exportDir ? `${String(n).padStart(2, "0")}_${s.category}_${n}.jpg` : null,
            image: {
              id: n,
              style: "檀健次款",
              category: s.category,
              name: "图",
              filePath: "/库/图.jpg",
            },
          };
        }),
      );
      return {
        packId: "pack-test",
        markedUsed: markUsed || Boolean(exportDir),
        picks,
        notes: ["礼盒图：降级到 3 天冷却"],
        exportDir: exportDir ?? null,
        manifestPath: exportDir ? path.join(exportDir, "素材包.json") : null,
      };
    },
    "library.fix-orientation": ({ dryRun }) => ({
      dryRun,
      scanned: 3,
      needsFix: 1,
      repaired: dryRun ? 0 : 1,
      alreadyUpright: 0,
      missing: [],
      failed: [],
      fixes: [
        { refImageId: 9, name: "竖拍", stored: [8064, 6048], display: [6048, 8064], inUse: false },
      ],
    }),
    "intake.confirm": ({ jobId }) => {
      const job = { ...jobs.get(jobId), status: "done" };
      jobs.set(jobId, job);
      return { job };
    },
    overview: () => ({ tasks: { queued: 1, running: 0, review: 2, failed: 0 }, paused: false }),
    "v2v.pending": () => ({ groups: [], skipped: 0 }),
    "v2v.rewrite": ({ lines }) => ({ applied: lines.length }),
    "poster.resolve": ({ style }) => ({
      autoImport: true,
      targets: [
        style === "檀健次款"
          ? {
              product: "2027明星日历",
              style,
              targetId: "tan",
              targetName: "檀健次日历（2027）",
              warning: "",
            }
          : {
              product: "2027明星日历",
              style,
              targetId: null,
              targetName: null,
              warning: "尚未绑定款式",
            },
      ],
    }),
    "poster.bind": ({ product, style, target }) => ({
      autoImport: true,
      target: { product, style, targetId: "x", targetName: target, warning: "" },
    }),
    "poster.backfill": ({ apply }) => ({
      applied: apply,
      autoImport: true,
      groups: [
        {
          product: "2027明星日历",
          style: "檀健次款",
          targetId: "tan",
          targetName: "檀健次日历（2027）",
          warning: "",
          count: 3,
          missingFiles: 0,
          batches: apply ? 1 : 0,
        },
      ],
      images: apply ? 3 : 0,
      batches: apply ? 1 : 0,
    }),
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
      // Library references ({"category": …}) have no local file to prepare.
      for (const input of (item.refs ?? [item.ref]).filter((r) => typeof r === "string")) {
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

test("manifest library references pass through, are checked by validate and echoed by submit", async (t) => {
  const f = await fixture(t);
  const manifest = path.join(f.input, "图库清单.json");
  const spec = {
    version: 1,
    organization: { product: "", style: "王一博", requirement: "场景图", source: "CLI" },
    items: [
      {
        name: "场景一",
        promptFile: "桌面场景.txt",
        refs: [{ category: "正面图" }, "商品实拍.png", { libraryId: 7 }],
      },
    ],
  };
  await fs.writeFile(manifest, JSON.stringify(spec));
  // The validate-intake double reports no librarySlots, so only the structure is checked here.
  const validated = await f.run("validate", "--manifest", manifest);
  assert.equal(validated.code, 0, validated.data.message);
  const submitted = await f.run("submit", "--manifest", manifest);
  assert.equal(submitted.code, 0, submitted.data.message);
  assert.deepEqual(
    submitted.data.libraryPicks.map((p) => [p.item, p.slot, p.imageId]),
    [
      ["场景一", 1, 100],
      ["场景一", 3, 101],
    ],
  );
  const [archived] = await fs.readdir(f.consumed);
  const order = JSON.parse(await fs.readFile(path.join(f.consumed, archived, "工单.json"), "utf8"));
  assert.deepEqual(order.items[0].refs[0], { category: "正面图" });
  assert.match(order.items[0].refs[1], /^refs\/[a-f0-9]{64}\.png$/);
  assert.deepEqual(order.items[0].refs[2], { libraryId: 7 });
  // Same manifest, same jobId: no second draw.
  const again = await f.run("submit", "--manifest", manifest);
  assert.equal(again.data.jobId, submitted.data.jobId);
  assert.equal(again.data.reused, true);

  for (const bad of [
    [{ categroy: "正面图" }],
    [{ libraryId: 7, category: "正面图" }],
    [{ libraryId: 7 }, { libraryId: 7 }],
    [{ libraryId: 0 }],
  ]) {
    spec.items[0].refs = bad;
    await fs.writeFile(manifest, JSON.stringify(spec));
    assert.equal((await f.run("validate", "--manifest", manifest)).code, 2, JSON.stringify(bad));
  }
});

test("validate fails when the running app reports an empty library category", async (t) => {
  const f = await fixture(t);
  await fs.writeFile(
    f.mock,
    `#!/usr/bin/env node
import fs from "node:fs";
const input = JSON.parse(fs.readFileSync(0, "utf8"));
process.stdout.write(JSON.stringify({status:"validated",validation:"full",protocolVersion:1,
  taskCount:input.order.items.length,refCount:0,
  librarySlots:[{item:"甲",slot:1,reference:"图库 王一博/礼盒图",spec:{style:"王一博",product:"",category:"礼盒图"}}]}));
`,
  );
  const manifest = path.join(f.input, "礼盒清单.json");
  await fs.writeFile(
    manifest,
    JSON.stringify({
      version: 1,
      organization: { product: "", style: "王一博", requirement: "礼盒", source: "CLI" },
      items: [{ name: "甲", promptFile: "桌面场景.txt", ref: { category: "礼盒图" } }],
    }),
  );
  const result = await f.run("validate", "--manifest", manifest);
  assert.equal(result.code, 2);
  assert.match(result.data.message, /礼盒图/);
  assert.ok(commands(f).includes("library.check"));
});

test("library commands forward to the app and refuse an app without the feature", async (t) => {
  const f = await fixture(t);
  const folder = path.join(f.input, "王一博正面");
  await fs.mkdir(folder);
  await fs.writeFile(path.join(folder, "b.png"), png);
  await fs.writeFile(path.join(folder, "a.jpg"), png);
  await fs.writeFile(path.join(folder, "说明.txt"), "不是图片");
  const imported = await f.run(
    "library",
    "import",
    "--style",
    "王一博",
    "--category",
    "正面图",
    "--dir",
    folder,
  );
  assert.equal(imported.code, 0, imported.data.message);
  assert.equal(imported.data.imported, 2);
  const importCall = f.calls.find((c) => c.cmd === "library.import");
  assert.deepEqual(
    importCall.args.paths.map((p) => path.basename(p)),
    ["a.jpg", "b.png"],
  );
  assert.equal(importCall.args.style, "王一博");

  const picked = await f.run(
    "library",
    "pick",
    "--style",
    "王一博",
    "--category",
    "正面图",
    "--count",
    "3",
  );
  assert.equal(picked.code, 0);
  assert.equal(picked.data.picks.length, 3);
  assert.equal(picked.data.markedUsed, false);
  const updated = await f.run("library", "update", "--id", "3", "--id", "4", "--disable");
  assert.deepEqual(f.calls.at(-1).args, { ids: [3, 4], enabled: false });
  assert.equal(updated.data.changed, 2);
  assert.equal((await f.run("library", "update", "--id", "3")).code, 2);
  assert.equal(
    (await f.run("library", "pick", "--style", "王一博", "--category", "正面图", "--count", "0"))
      .code,
    2,
  );
  assert.equal((await f.run("library", "overview", "--dir", folder)).code, 2);
  assert.equal((await f.run("library", "nope")).code, 2);

  const old = await fixture(t, { features: [] });
  const refused = await old.run("library", "overview");
  assert.equal(refused.code, 8);
  assert.match(refused.data.message, /reference-library/);
});

test("library pack takes --slot or --spec, previews by default and exports when asked", async (t) => {
  const f = await fixture(t);
  const preview = await f.run(
    "library",
    "pack",
    "--style",
    "檀健次款",
    "--product",
    "2027明星日历",
    "--slot",
    "正面图:2",
    "--slot",
    "45度角：2",
    "--slot",
    "礼盒图",
  );
  assert.equal(preview.code, 0, preview.data.message);
  assert.deepEqual(f.calls.at(-1).args, {
    style: "檀健次款",
    product: "2027明星日历",
    slots: [
      { category: "正面图", count: 2 },
      { category: "45度角", count: 2 },
      { category: "礼盒图", count: 1 },
    ],
    markUsed: false,
  });
  assert.equal(preview.data.markedUsed, false);
  assert.deepEqual(
    preview.data.refs,
    [1, 2, 3, 4, 5].map((libraryId) => ({ libraryId })),
    "预览给出可直接写进工单的 refs",
  );
  assert.equal(preview.data.picks[4].cooldownDays, 3);

  const specFile = path.join(f.input, "素材包.json");
  await fs.writeFile(
    specFile,
    JSON.stringify({
      style: "檀健次款",
      slots: [
        { category: "正面图", count: 2 },
        { category: "内页展示图", count: 2 },
      ],
    }),
  );
  const out = path.join(f.home, "导出");
  const exported = await f.run("library", "pack", "--spec", specFile, "--export", out);
  assert.equal(exported.code, 0, exported.data.message);
  assert.equal(f.calls.at(-1).args.exportDir, out);
  assert.equal(exported.data.markedUsed, true);
  assert.equal(exported.data.refs, undefined, "已记账就不再给 libraryId，免得重复记账");
  assert.equal(exported.data.picks[0].file, "01_正面图_1.jpg");

  for (const bad of [
    ["library", "pack", "--style", "檀健次款"],
    ["library", "pack", "--spec", specFile, "--slot", "正面图"],
    ["library", "pack", "--style", "檀健次款", "--slot", "正面图:0"],
  ])
    assert.equal((await f.run(...bad)).code, 2, bad.join(" "));
  await fs.writeFile(specFile, JSON.stringify({ style: "x", slots: [{ categroy: "正面图" }] }));
  assert.equal((await f.run("library", "pack", "--spec", specFile)).code, 2);
});

test("library fix-orientation forwards dry-run and summarises the fixes", async (t) => {
  const f = await fixture(t);
  const dry = await f.run("library", "fix-orientation", "--dry-run");
  assert.equal(dry.code, 0, dry.data.message);
  assert.deepEqual(f.calls.at(-1).args, { dryRun: true });
  assert.deepEqual(dry.data.fixes[0], {
    refImageId: 9,
    name: "竖拍",
    stored: "8064x6048",
    display: "6048x8064",
  });
  const done = await f.run("library", "fix-orientation");
  assert.equal(done.data.repaired, 1);
});

test("manifest library references can ask for several distinct images with count", async (t) => {
  const f = await fixture(t);
  const manifest = path.join(f.input, "素材包清单.json");
  const spec = {
    version: 1,
    organization: {
      product: "2027明星日历",
      style: "檀健次款",
      requirement: "场景图",
      source: "CLI",
    },
    items: [
      {
        name: "场景一",
        promptFile: "桌面场景.txt",
        refs: [{ category: "正面图", count: 2 }, { category: "礼盒图" }],
      },
    ],
  };
  await fs.writeFile(manifest, JSON.stringify(spec));
  const submitted = await f.run("submit", "--manifest", manifest);
  assert.equal(submitted.code, 0, submitted.data.message);
  const [archived] = await fs.readdir(f.consumed);
  const order = JSON.parse(await fs.readFile(path.join(f.consumed, archived, "工单.json"), "utf8"));
  assert.deepEqual(order.items[0].refs, [
    { category: "正面图" },
    { category: "正面图" },
    { category: "礼盒图" },
  ]);
  for (const bad of [
    [{ category: "正面图", count: 0 }],
    [{ libraryId: 7, count: 2 }],
    [{ category: "正面图", count: 17 }],
  ]) {
    spec.items[0].refs = bad;
    await fs.writeFile(manifest, JSON.stringify(spec));
    assert.equal((await f.run("validate", "--manifest", manifest)).code, 2, JSON.stringify(bad));
  }
});

test("poster commands resolve, bind and backfill through the app", async (t) => {
  const f = await fixture(t);
  const known = await f.run(
    "poster",
    "resolve",
    "--product",
    "2027明星日历",
    "--style",
    "檀健次款",
  );
  assert.equal(known.code, 0, known.data.message);
  assert.equal(known.data.targetName, "檀健次日历（2027）");
  const unknown = await f.run(
    "poster",
    "resolve",
    "--product",
    "2027明星日历",
    "--style",
    "谢霆锋演唱会款",
  );
  assert.equal(unknown.code, 4);
  assert.match(unknown.data.next, /gendesk poster bind/);

  const bound = await f.run(
    "poster",
    "bind",
    "--product",
    "2027明星日历",
    "--style",
    "谢霆锋演唱会款",
    "--target",
    "谢霆锋日历（2027）",
  );
  assert.equal(bound.code, 0, bound.data.message);
  assert.deepEqual(f.calls.at(-1).args, {
    product: "2027明星日历",
    style: "谢霆锋演唱会款",
    target: "谢霆锋日历（2027）",
  });
  assert.equal((await f.run("poster", "bind", "--style", "谢霆锋演唱会款")).code, 2);

  const dry = await f.run("poster", "backfill");
  assert.equal(dry.code, 0);
  assert.equal(dry.data.applied, false);
  assert.deepEqual(f.calls.at(-1).args, { product: "", style: "", apply: false });
  const applied = await f.run("poster", "backfill", "--style", "檀健次款", "--apply");
  assert.equal(applied.data.images, 3);
  assert.equal(f.calls.at(-1).args.apply, true);
  assert.equal((await f.run("poster", "backfill", "--target", "x")).code, 2);
  assert.equal((await f.run("poster", "nope")).code, 2);

  const old = await fixture(t, { features: ["reference-library"] });
  const refused = await old.run("poster", "resolve", "--style", "檀健次款");
  assert.equal(refused.code, 8);
  assert.match(refused.data.message, /poster-link/);
});
