#!/usr/bin/env node
// File-based client only. Imports, scheduling, keys and review stay inside GenDesk's Rust backend.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import * as fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { parseArgs } from "node:util";

// This file is installed standalone. The installer and tests check the release version.
const CLI_VERSION = "0.36.1";
const ORDER_VERSION = 1;
const CLIENT_FEATURES = [
  "ordered-references",
  "generation-quality",
  "content-organization",
  "intake-preflight",
  "resumable-intake",
  "background-intake",
];

const HELP = `GenDesk CLI — 提示词文件 + 参考图路径 → 生图 → GenDesk 人工验收

gendesk --version
gendesk doctor [--root <交接根>]
gendesk validate --manifest <任务清单.json>
gendesk submit --manifest <任务清单.json> [--open] [--wait 30]
gendesk submit --prompt <正文.txt> --ref <参考图>
               --product <产品> --style <款式> --requirement <需求> [--source <来源>]
               [--name <图片语义名>] [--ratio 3:4] [--format jpeg] [--quality high] [--open] [--wait 30]
gendesk status <jobId> [--root <交接根>] [--wait 30]

通用：--root（或 GENDESK_ROOT）指定交接根；--job-id 指定去重键。
单条参数：--name --sku --style --product --requirement --source --note --purpose i2v --skill --ephemeral
          --ratio --size --format --quality（默认 high）
每个正文文件视为一条提示词；多条用 --manifest 显式配对。
清单中的 promptFile/ref/refs 相对清单文件所在目录解析，可用绝对路径和 ~/。
多参考图用清单 refs（有序 1～16 张），与 ref 二选一；一项仍只生成一张图。
organization 按产品/款式/需求持续归档，条目整对象覆盖顶层；无 SKU 的任务不虚构编号。
不填 organization 时兼容旧 --sku <SKU> --style <款式>，二者须非空。
批量清单示例：
{"version":1,"organization":{"product":"陶瓷水杯","style":"奶油白款","requirement":"厨房场景图","source":"CLI"},
 "defaults":{"ratio":"3:4","format":"jpeg","quality":"high"},"items":[
  {"name":"水杯晨光桌面","promptFile":"正文.txt","refs":["商品.jpg","桌面.jpg"]}
]}
validate 调用应用 Rust 只读完整预检（参数取值、实际图片解码与归属），不投单、不生图。
需应用声明 intake-preflight；可用 GENDESK_APP_BIN 指定已知支持预检的应用可执行文件。
doctor 分别报告 cliVersion、appVersion 和连接能力；连接快照不代表应用在线。
macOS 的 --open 只在需要时后台启动 GenDesk，生成结果可稍后手动打开验收。
所有结果/错误均为 JSON。--wait 只等待收录回执，不代表生图或验收完成。
退出码：0 成功/已投递；2 输入错误；3 等待超时；4 待确认；5 收录失败；
        6 重名未收录；7 未找到；8 收件已关闭。
相同内容默认使用相同 jobId，防止重放计费；有意重跑请给新的 --job-id。`;

class CliError extends Error {
  constructor(message, code = 2) {
    super(message);
    this.code = code;
  }
}

function check(condition, message) {
  if (!condition) throw new CliError(message);
}

function filePath(value, base = process.cwd()) {
  if (value === "~") return os.homedir();
  return path.resolve(
    base,
    value.startsWith("~/") ? path.join(os.homedir(), value.slice(2)) : value,
  );
}

async function optionalJson(file) {
  try {
    return JSON.parse(await fs.readFile(file, "utf8"));
  } catch (e) {
    if (e.code === "ENOENT") return null;
    throw new CliError(`无法读取 JSON ${file}：${e.message}`);
  }
}

function object(value, label) {
  check(value && typeof value === "object" && !Array.isArray(value), `${label} 必须是对象`);
  return value;
}

function knownKeys(value, keys, label) {
  object(value, label);
  for (const key of Object.keys(value)) check(keys.includes(key), `${label} 不支持字段 ${key}`);
}

function requiredText(value, label) {
  check(typeof value === "string" && value.trim(), `${label} 必须是非空字符串`);
  return value.trim();
}

function safeName(value, label) {
  const name = requiredText(value, label);
  check(
    !/[<>:"/\\|?*]/.test(name) &&
      ![...name].some(
        (c) => c.charCodeAt(0) < 32 || (c.charCodeAt(0) >= 127 && c.charCodeAt(0) <= 159),
      ) &&
      !/[. ]$/.test(name) &&
      !/^\.+$/.test(name),
    `${label} 不能包含路径分隔符或文件系统保留字符`,
  );
  check(!/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(name), `${label} 是保留文件名`);
  check(Buffer.byteLength(name) <= 180, `${label} 过长`);
  return name;
}

const PARAM_KEYS = ["ratio", "size", "format", "quality", "clearAiMetadata", "removeC2pa"];
const QUALITIES = ["auto", "low", "medium", "high", "xhigh", "max", "standard", "hd"];
function parameters(value, label) {
  knownKeys(value, PARAM_KEYS, label);
  for (const [key, entry] of Object.entries(value)) {
    if (key === "clearAiMetadata" || key === "removeC2pa") {
      check(typeof entry === "boolean", `${label}.${key} 必须是布尔值`);
    } else requiredText(entry, `${label}.${key}`);
  }
  if (value.quality !== undefined) {
    value.quality = value.quality.trim().toLowerCase();
    check(
      QUALITIES.includes(value.quality),
      `${label}.quality 不支持 ${value.quality}；可用：${QUALITIES.join(" / ")}`,
    );
  }
  return value;
}

async function digestFile(file) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest("hex");
}

async function readableFile(value, base, label) {
  const file = filePath(requiredText(value, label), base);
  const stat = await fs.stat(file);
  check(stat.isFile() && stat.size > 0, `${label} 不是非空文件：${file}`);
  return file;
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonical(value[key])]),
    );
  }
  return value;
}

async function prepare(opts) {
  let spec;
  let base = process.cwd();
  const singleOptions = [
    "prompt",
    "ref",
    "name",
    "sku",
    "style",
    "ratio",
    "size",
    "format",
    "quality",
    "note",
    "purpose",
    "skill",
    "ephemeral",
    "product",
    "requirement",
    "source",
  ];
  if (opts.manifest) {
    check(
      !singleOptions.some((key) => opts[key] !== undefined),
      "--manifest 不能与单条参数混用；请在清单中设置",
    );
    const manifest = filePath(opts.manifest);
    base = path.dirname(manifest);
    spec = await optionalJson(manifest);
    check(spec, `清单不存在：${manifest}`);
  } else {
    const prompt = requiredText(opts.prompt, "--prompt");
    spec = {
      note: opts.note,
      ...(opts.product !== undefined || opts.requirement !== undefined || opts.source !== undefined
        ? {
            organization: {
              product: opts.product ?? "",
              style: opts.style ?? "",
              requirement: opts.requirement ?? opts.note ?? "",
              source: opts.source ?? "CLI",
            },
          }
        : {}),
      purpose: opts.purpose,
      skill: opts.skill,
      ephemeral: opts.ephemeral,
      defaults: Object.fromEntries(
        ["ratio", "size", "format", "quality"]
          .filter((k) => opts[k] !== undefined)
          .map((k) => [k, opts[k]]),
      ),
      items: [
        {
          name: opts.name ?? path.parse(prompt).name,
          sku: opts.sku,
          style: opts.style,
          promptFile: prompt,
          ref: opts.ref,
        },
      ],
    };
    // Keep absent fields absent in the wire order and the content digest.
    spec = JSON.parse(JSON.stringify(spec));
  }
  knownKeys(
    spec,
    [
      "version",
      "jobId",
      "note",
      "purpose",
      "skill",
      "defaults",
      "items",
      "refGroup",
      "ephemeral",
      "organization",
    ],
    "清单",
  );
  check(spec.version === undefined || spec.version === 1, "清单 version 只支持 1");
  for (const key of ["jobId", "note", "purpose", "skill", "refGroup"]) {
    if (spec[key] !== undefined) requiredText(spec[key], key);
  }
  check(
    spec.ephemeral === undefined || typeof spec.ephemeral === "boolean",
    "ephemeral 必须是布尔值",
  );
  function organization(value, label) {
    if (value === undefined) return undefined;
    knownKeys(value, ["product", "style", "requirement", "source"], label);
    for (const [key, text] of Object.entries(value)) {
      check(typeof text === "string", `${label}.${key} 必须是字符串`);
      if (text && key !== "source") {
        check(text === text.trim(), `${label}.${key} 不能包含首尾空白`);
        safeName(text, `${label}.${key}`);
      }
    }
    return value;
  }
  organization(spec.organization, "organization");
  const defaults = { quality: "high", ...parameters(spec.defaults ?? {}, "defaults") };
  check(Array.isArray(spec.items) && spec.items.length > 0, "items 必须包含至少一个条目");
  const seen = new Set();
  const files = [];
  const refs = new Map();
  const items = [];
  for (const [index, item] of spec.items.entries()) {
    const label = `items[${index}]`;
    knownKeys(
      item,
      ["name", "sku", "style", "promptFile", "ref", "refs", "organization", ...PARAM_KEYS],
      label,
    );
    const name = safeName(item.name, `${label}.name`);
    const identity = name.normalize("NFC").toLowerCase();
    check(!seen.has(identity), `条目名重复（含大小写或 Unicode 等价）：${name}`);
    seen.add(identity);
    const ownership = item.organization ?? spec.organization;
    const sku = ownership ? (item.sku ?? "") : requiredText(item.sku, `${label}.sku`);
    const style = ownership
      ? (item.style ?? ownership.style ?? "")
      : requiredText(item.style, `${label}.style`);
    check(typeof sku === "string" && typeof style === "string", `${label}.sku/style 必须是字符串`);
    const promptFile = await readableFile(item.promptFile, base, `${label}.promptFile`);
    const bytes = await fs.readFile(promptFile);
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    check(text.trim(), `提示词为空：${promptFile}`);
    check(
      !(item.ref !== undefined && item.refs !== undefined),
      `${label} 的 ref 与 refs 只能填写一种`,
    );
    const inputs = item.refs === undefined ? [item.ref] : item.refs;
    check(
      Array.isArray(inputs) && inputs.length > 0 && inputs.length <= 16,
      `${label}.refs 必须包含 1～16 张有序参考图`,
    );
    const orderedRefs = [];
    for (const input of inputs) {
      const source = await fs.realpath(await readableFile(input, base, `${label}.refs`));
      if (!refs.has(source)) {
        const hash = await digestFile(source);
        const relative = `refs/${hash}${path.extname(source).toLowerCase()}`;
        refs.set(source, { source, relative, hash });
      }
      const relative = refs.get(source).relative;
      check(!orderedRefs.includes(relative), `${label} 重复引用同一张图`);
      orderedRefs.push(relative);
    }
    const params = parameters(
      Object.fromEntries(
        PARAM_KEYS.filter((key) => item[key] !== undefined).map((key) => [key, item[key]]),
      ),
      label,
    );
    items.push({
      name,
      sku,
      style,
      ...(item.organization !== undefined
        ? { organization: organization(item.organization, `${label}.organization`) }
        : {}),
      ...(orderedRefs.length === 1 ? { ref: orderedRefs[0] } : { refs: orderedRefs }),
      ...params,
    });
    files.push({ name, bytes });
  }
  const order = { ...spec, version: 1, defaults, items };
  order.jobId = undefined;
  const hash = createHash("sha256")
    .update(
      JSON.stringify(canonical({ order, prompts: files.map((f) => f.bytes.toString("base64")) })),
    )
    .digest("hex");
  // A stable content key survives harness retries, app restarts and midnight.
  order.jobId = safeName(opts["job-id"] ?? spec.jobId ?? `cli-${hash.slice(0, 32)}`, "jobId");
  check(!order.jobId.startsWith("_"), "jobId 不能以下划线开头（GenDesk 会跳过内部目录）");
  const distinctRefs = new Map([...refs.values()].map((ref) => [ref.relative, ref]));
  return { order, files, refs: [...distinctRefs.values()], hash };
}

async function connection(opts) {
  const defaultRoot = path.join(os.homedir(), "GenDesk交接");
  const discoveryPath = path.join(defaultRoot, "连接.json");
  const discovery = await optionalJson(discoveryPath);
  if (discovery)
    check(
      discovery.version === 1 && typeof discovery.root === "string",
      `不支持的连接信息：${discoveryPath}`,
    );
  const override = opts.root ?? process.env.GENDESK_ROOT;
  if (override !== undefined) requiredText(override, "交接根");
  const root = filePath(override ?? discovery?.root ?? defaultRoot);
  const matches = discovery && filePath(discovery.root) === root;
  return {
    root,
    pendingDir: path.join(root, "生图", "收件"),
    consumedDir: path.join(root, "生图", "_已收录"),
    source: opts.root
      ? "--root"
      : process.env.GENDESK_ROOT
        ? "GENDESK_ROOT"
        : discovery
          ? "连接.json"
          : "default",
    enabled: matches ? discovery.enabled : null,
    taskThreshold: matches ? discovery.taskThreshold : null,
    appVersion: matches ? discovery.appVersion : null,
    features: matches && Array.isArray(discovery.features) ? discovery.features : [],
    validatorPath: matches ? discovery.validatorPath : null,
  };
}

async function nativePreflight(conn, prepared) {
  const override = process.env.GENDESK_APP_BIN;
  check(
    override || conn.features.includes("intake-preflight"),
    "当前应用未声明 Rust 完整预检支持，请先更新并启动 GenDesk；不会降级成文件预检",
  );
  const binary = override ?? conn.validatorPath;
  check(
    typeof binary === "string" && binary.trim(),
    "没有 Rust 预检程序路径，请启动新版应用刷新连接信息",
  );
  const input = {
    version: 1,
    order: prepared.order,
    prompts: Object.fromEntries(
      prepared.files.map((file) => [file.name, file.bytes.toString("utf8")]),
    ),
    references: Object.fromEntries(prepared.refs.map((ref) => [ref.relative, ref.source])),
  };
  const result = spawnSync(filePath(binary), ["validate-intake"], {
    input: JSON.stringify(input),
    encoding: "utf8",
    timeout: 300000,
    maxBuffer: 32 * 1024 * 1024,
  });
  if (result.error) throw new CliError(`Rust 预检调用失败：${result.error.message}`);
  let report;
  try {
    report = JSON.parse(result.stdout);
  } catch {
    throw new CliError(`Rust 预检返回无效 JSON：${result.stderr || result.stdout}`);
  }
  check(result.status === 0, report.message ?? "Rust 完整预检失败");
  check(
    report.protocolVersion === 1 &&
      report.validation === "full" &&
      report.taskCount === prepared.order.items.length,
    "Rust 预检协议或任务数不匹配，未投递",
  );
  return report;
}

async function directories(parent) {
  try {
    return (await fs.readdir(parent, { withFileTypes: true }))
      .filter((e) => e.isDirectory() && !e.name.startsWith("_"))
      .map((e) => path.join(parent, e.name));
  } catch (e) {
    if (e.code === "ENOENT") return [];
    throw e;
  }
}

async function locate(conn, jobId) {
  // Pending first: a new delivery with a reused ID can have a duplicate receipt.
  for (const parent of [conn.pendingDir, conn.consumedDir]) {
    for (const dir of (await directories(parent)).sort().reverse()) {
      let order;
      try {
        order = await optionalJson(path.join(dir, "工单.json"));
      } catch {
        // Another harness may still be writing, or an unrelated order may be invalid.
        // Neither should make every other job impossible to submit or query.
        if (path.basename(dir) === jobId) return dir;
        continue;
      }
      if ((order?.jobId ?? path.basename(dir)) === jobId) return dir;
    }
  }
  return null;
}

async function textIfExists(file) {
  try {
    return await fs.readFile(file, "utf8");
  } catch (e) {
    if (e.code === "ENOENT") return null;
    throw e;
  }
}

async function status(conn, jobId) {
  const dir = await locate(conn, jobId);
  if (!dir) return { jobId, status: "not_found", root: conn.root };
  const receipt = await optionalJson(path.join(dir, "回执.json"));
  if (receipt) {
    check(receipt.version === 1 && receipt.job?.jobId === jobId, `回执格式或 jobId 不匹配：${dir}`);
    return {
      jobId,
      status: { done: "ingested", running: "ingesting" }[receipt.job.status] ?? receipt.job.status,
      dir,
      receipt: receipt.job,
      next: "生成及人工验收状态请在 GenDesk 查看；本回执只表示工单收录状态",
    };
  }
  // Text receipt fallback only. Older apps with the legacy order format still need updating.
  for (const [file, state] of [
    ["结果.txt", "ingested"],
    ["错误.txt", "error"],
    ["待确认.txt", "hold"],
    ["重名未收录.txt", "duplicate"],
  ]) {
    const message = await textIfExists(path.join(dir, file));
    if (message !== null)
      return {
        jobId,
        status: state,
        dir,
        message,
        receiptFormat: "legacy-text",
        next: "生成及人工验收状态请在 GenDesk 查看",
      };
  }
  if ((await textIfExists(path.join(dir, "READY.txt"))) === null) {
    return {
      jobId,
      status: "staging",
      dir,
      message:
        "尚无 READY，工单未发布。客户端可能仍在写入；若已退出，请先检查这份半成品，不要换 ID 重投",
    };
  }
  return {
    jobId,
    status: "queued",
    dir,
    message: "尚无收录回执；请确认 GenDesk 已启动、收件已开启且交接根一致",
  };
}

async function waitStatus(conn, jobId, seconds) {
  const deadline = Date.now() + seconds * 1000;
  for (;;) {
    const result = await status(conn, jobId);
    if (!["queued", "staging", "ingesting", "not_found"].includes(result.status) || seconds === 0)
      return result;
    if (Date.now() >= deadline) return { ...result, timedOut: true };
    await delay(Math.min(500, Math.max(1, deadline - Date.now())));
  }
}

function exitCode(result) {
  if (result.timedOut) return 3;
  return { staging: 2, hold: 4, error: 5, duplicate: 6, not_found: 7 }[result.status] ?? 0;
}

function startAppInBackground(conn) {
  const running = spawnSync("/usr/bin/lsappinfo", ["find", "bundleID=com.gendesk.app"], {
    encoding: "utf8",
  });
  check(
    !running.error && running.status === 0,
    `无法检查 GenDesk 是否在运行：${running.error?.message ?? running.stderr}`,
  );
  // An already running app watches the inbox. An open request could reopen its hidden window.
  if (running.stdout.trim()) return;
  check(
    conn.features.includes("background-intake"),
    "当前应用尚不支持后台启动，请更新 GenDesk；工单已投递",
  );
  const bundle = path.resolve(conn.validatorPath ?? "", "../../..");
  check(path.extname(bundle) === ".app", "未找到支持后台启动的 GenDesk 应用包；工单已投递");
  // Address the inbox so simultaneous launches deliver a file-open event, not Reopen.
  const result = spawnSync(
    "/usr/bin/open",
    ["-g", "-a", bundle, conn.pendingDir, "--args", "--background-intake"],
    { encoding: "utf8" },
  );
  if (result.error || result.status !== 0)
    throw new CliError(`启动 GenDesk 失败：${result.error?.message ?? result.stderr}`);
}

async function submit(conn, prepared) {
  if (conn.enabled === false) throw new CliError("GenDesk 收件已关闭，请在设置中开启后再提交", 8);
  const { order, files, refs, hash } = prepared;
  if (order.items.some((item) => item.refs?.length > 1)) {
    check(
      conn.features.includes("ordered-references"),
      "当前连接信息未声明双参考图支持，请先更新并启动 GenDesk，再投递此工单",
    );
  }
  check(
    conn.features.includes("generation-quality"),
    "当前连接信息未声明 quality 支持，请先更新并启动 GenDesk，避免质量参数被旧应用忽略",
  );
  check(
    conn.features.includes("resumable-intake"),
    "当前应用未声明安全恢复支持，请先更新并启动 GenDesk；未投递工单",
  );
  if (order.organization || order.items.some((item) => item.organization)) {
    check(
      conn.features.includes("content-organization"),
      "当前应用不支持产品、款式、需求归档，请先更新并启动 GenDesk",
    );
  }
  const found = await locate(conn, order.jobId);
  if (found) {
    const client = await optionalJson(path.join(found, "客户端.json"));
    check(
      client?.contentHash === hash,
      `jobId 已存在且内容不同或不是 CLI 工单：${order.jobId}。请核对，不会覆盖或重投`,
    );
    return { ...(await status(conn, order.jobId)), reused: true };
  }
  await nativePreflight(conn, prepared);
  await fs.mkdir(conn.pendingDir, { recursive: true });
  const dir = path.join(conn.pendingDir, order.jobId);
  try {
    // mkdir without recursive is the inter-process claim for this ID. Never overwrite it.
    await fs.mkdir(dir);
  } catch (e) {
    if (e.code === "EEXIST")
      throw new CliError(`同一 jobId 正在提交：${order.jobId}。请用 status 查询，不会覆盖`);
    throw e;
  }
  // On failure leave the partial directory WITHOUT READY, so the app cannot ingest it.
  await fs.mkdir(path.join(dir, "prompts"));
  await fs.mkdir(path.join(dir, "refs"));
  for (const ref of refs) {
    const target = path.join(dir, ref.relative);
    await fs.copyFile(ref.source, target);
    check(
      (await digestFile(target)) === ref.hash,
      `复制过程中参考图发生变化：${ref.source}。未提交，请重新检查输入`,
    );
  }
  for (const file of files)
    await fs.writeFile(path.join(dir, "prompts", `${file.name}.txt`), file.bytes, { flag: "wx" });
  await fs.writeFile(path.join(dir, "工单.json"), `${JSON.stringify(order, null, 2)}\n`, {
    flag: "wx",
  });
  await fs.writeFile(
    path.join(dir, "客户端.json"),
    `${JSON.stringify({ version: 1, contentHash: hash })}\n`,
    { flag: "wx" },
  );
  await fs.writeFile(path.join(dir, "READY.txt"), "ready\n", { flag: "wx" });
  return {
    jobId: order.jobId,
    status: "queued",
    dir,
    taskCount: order.items.length,
    refCount: refs.length,
    reused: false,
  };
}

async function main() {
  const { values: opts, positionals } = parseArgs({
    allowPositionals: true,
    options: Object.fromEntries([
      ...[
        "root",
        "manifest",
        "prompt",
        "ref",
        "name",
        "sku",
        "style",
        "note",
        "purpose",
        "skill",
        "ratio",
        "size",
        "format",
        "quality",
        "product",
        "requirement",
        "source",
        "job-id",
        "wait",
      ].map((key) => [key, { type: "string" }]),
      ...["help", "version", "open", "ephemeral"].map((key) => [key, { type: "boolean" }]),
    ]),
  });
  if (opts.version) {
    check(!positionals.length && Object.keys(opts).length === 1, "--version 不能与其他参数混用");
    process.stdout.write(
      `${JSON.stringify({ cliVersion: CLI_VERSION, orderVersion: ORDER_VERSION, clientFeatures: CLIENT_FEATURES }, null, 2)}\n`,
    );
    return;
  }
  if (opts.help || !positionals.length) {
    process.stdout.write(`${HELP}\n`);
    return;
  }
  const [command, jobId] = positionals;
  check(
    ["doctor", "validate", "submit", "status"].includes(command),
    `未知命令：${command}；请运行 gendesk --help`,
  );
  check(
    positionals.length === (command === "status" ? 2 : 1),
    "命令位置参数不正确；请运行 gendesk --help",
  );
  const allowed =
    command === "doctor"
      ? ["root"]
      : command === "status"
        ? ["root", "wait"]
        : [
            "root",
            "manifest",
            "prompt",
            "ref",
            "name",
            "sku",
            "style",
            "note",
            "purpose",
            "skill",
            "ratio",
            "size",
            "format",
            "quality",
            "product",
            "requirement",
            "source",
            "job-id",
            "ephemeral",
            ...(command === "submit" ? ["wait", "open"] : []),
          ];
  for (const key of Object.keys(opts)) check(allowed.includes(key), `${command} 不支持 --${key}`);
  const seconds = Number(opts.wait ?? 0);
  check(Number.isFinite(seconds) && seconds >= 0 && seconds <= 3600, "--wait 必须为 0～3600 秒");
  const conn = await connection(opts);
  let result;
  if (command === "doctor") {
    result = {
      ...conn,
      cliVersion: CLI_VERSION,
      orderVersion: ORDER_VERSION,
      clientFeatures: CLIENT_FEATURES,
      status: "configured",
      nodeVersion: process.version,
      message:
        conn.enabled === null
          ? "旧版应用未提供连接信息；交接根需与 GenDesk 设置一致。无需电脑操作，可直接投文件工单"
          : "连接信息是上次启动/保存设置的快照，不代表应用在线",
      review: "生图结果进入 GenDesk 人工验收页",
    };
  } else if (command === "status") {
    result = await waitStatus(conn, requiredText(jobId, "jobId"), seconds);
  } else {
    const prepared = await prepare(opts);
    if (command === "validate") {
      const preflight = await nativePreflight(conn, prepared);
      result = {
        status: "validated",
        validation: "full",
        jobId: prepared.order.jobId,
        taskCount: prepared.order.items.length,
        refCount: prepared.refs.length,
        order: prepared.order,
        preflight,
        message:
          "Rust 完整预检通过（参数、归属、正文与实际图片解码）；未投递、未调用生图。收录时仍会重新核对输入",
      };
    } else {
      // Check platform before publishing any READY marker.
      if (opts.open)
        check(process.platform === "darwin", "--open 当前仅支持 macOS；请先启动 GenDesk，再提交");
      result = await submit(conn, prepared);
      if (opts.open && result.status === "queued") {
        try {
          startAppInBackground(conn);
        } catch (e) {
          result.openError = e.message;
        }
      }
      if (seconds > 0 && ["queued", "staging", "ingesting"].includes(result.status))
        result = {
          ...(await waitStatus(conn, prepared.order.jobId, seconds)),
          reused: result.reused,
          ...(result.openError ? { openError: result.openError } : {}),
        };
    }
  }
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  process.exitCode = exitCode(result);
}

main().catch((e) => {
  process.stdout.write(
    `${JSON.stringify({ status: "client_error", message: e.message }, null, 2)}\n`,
  );
  process.exitCode = e.code && Number.isInteger(e.code) ? e.code : 2;
});
