#!/usr/bin/env node
// Agent CLI for GenDesk. Talks to the running app over its local control channel
// (127.0.0.1 + per-launch token from ~/GenDesk交接/连接.json). Imports, scheduling,
// keys and review stay inside GenDesk's Rust backend; this client only packages
// orders and forwards commands.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import * as fs from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { parseArgs } from "node:util";

// This file is installed standalone. The installer and tests check the release version.
const CLI_VERSION = "0.37.0";
const PROTOCOL = 2;
const ORDER_VERSION = 1;

const HELP = `GenDesk CLI — Agent 经本机控制通道投单、查进度、写回视频改写

gendesk --version
gendesk doctor
gendesk validate --manifest <任务清单.json>
gendesk submit --manifest <任务清单.json>
gendesk submit --prompt <正文.txt> --ref <参考图>
               --product <产品> --style <款式> --requirement <需求> [--source <来源>]
               [--name <图片语义名>] [--ratio 3:4] [--format jpeg] [--quality high]
gendesk status <jobId>
gendesk confirm <jobId>      超阈值待确认的工单，仅在用户明确同意后调用
gendesk retry <jobId>        失败或中断的工单，只补未完成步骤
gendesk overview             任务各态数量与视频待办
gendesk v2v pending          待改写的图生视频条目（含缩略图路径与改写说明）
gendesk v2v rewrite --file <改写结果.json>

单条参数：--name --sku --style --product --requirement --source --note --purpose i2v --skill
          --ratio --size --format --quality（默认 high）--job-id
每个正文文件视为一条提示词；多条用 --manifest 显式配对。
清单中的 promptFile/ref/refs 相对清单文件所在目录解析，可用绝对路径和 ~/。
多参考图用清单 refs（有序 1～16 张），与 ref 二选一；一项仍只生成一张图。
organization 按产品/款式/需求持续归档，条目整对象覆盖顶层；无 SKU 的任务不虚构编号。
批量清单示例：
{"version":1,"organization":{"product":"陶瓷水杯","style":"奶油白款","requirement":"厨房场景图","source":"CLI"},
 "defaults":{"ratio":"3:4","format":"jpeg","quality":"high"},"items":[
  {"name":"水杯晨光桌面","promptFile":"正文.txt","refs":["商品.jpg","桌面.jpg"]}
]}
validate 调用应用二进制做只读完整预检（参数取值、实际图片解码与归属），不投单、不生图。
submit 在 GenDesk 未运行时于后台启动它（macOS），收录完成才返回；生成与人工验收在应用内。
改写结果文件：JSON 数组、{"lines":[…]} 或每行一个 JSON；每条 {"id":"W123","videoPrompt":"…"}。
所有结果/错误均为 JSON。
退出码：0 成功；2 输入错误；4 待确认；5 收录或应用处理失败；6 重名未收录；7 未找到；8 应用不可用。
相同内容默认使用相同 jobId，防止重放计费；有意重跑请给新的 --job-id。`;

class CliError extends Error {
  constructor(message, code = 2) {
    super(message);
    this.code = code;
  }
}

function check(condition, message, code = 2) {
  if (!condition) throw new CliError(message, code);
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
  const stat = await fs.stat(file).catch(() => null);
  check(stat?.isFile() && stat.size > 0, `${label} 不是非空文件：${file}`);
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

const SINGLE_OPTIONS = [
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
  "product",
  "requirement",
  "source",
];

async function prepare(opts) {
  let spec;
  let base = process.cwd();
  if (opts.manifest) {
    check(
      !SINGLE_OPTIONS.some((key) => opts[key] !== undefined),
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
    ["version", "jobId", "note", "purpose", "skill", "defaults", "items", "organization"],
    "清单",
  );
  check(spec.version === undefined || spec.version === 1, "清单 version 只支持 1");
  for (const key of ["jobId", "note", "purpose", "skill"]) {
    if (spec[key] !== undefined) requiredText(spec[key], key);
  }
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
    let text;
    try {
      text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      throw new CliError(`提示词不是 UTF-8：${promptFile}`);
    }
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
  const order = { ...spec, version: ORDER_VERSION, defaults, items };
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

// ---------------------------------------------------------------- 连接

const ROOT = path.join(os.homedir(), "GenDesk交接");
const CONNECTION = path.join(ROOT, "连接.json");
const pendingDir = (root) => path.join(root, "生图", "收件");
const consumedDir = (root) => path.join(root, "生图", "_已收录");

/** 读连接文件；不存在返回 null，格式或协议不对直接报错。 */
async function readConnection() {
  let conn;
  try {
    conn = await optionalJson(CONNECTION);
  } catch (e) {
    throw new CliError(`连接文件损坏：${CONNECTION}。重启 GenDesk 会重写它（${e.message}）`, 8);
  }
  if (!conn) return null;
  check(
    conn.protocol === PROTOCOL && Number.isInteger(conn.port) && typeof conn.token === "string",
    `连接文件来自不兼容的 GenDesk（协议 ${conn.protocol ?? "旧版"}，本 CLI 需要 ${PROTOCOL}）。请更新并重启 GenDesk`,
    8,
  );
  return conn;
}

/** 一次请求 = 一次连接：写一行 JSON，读到对端关闭为止。 */
function rpc(conn, cmd, args = {}) {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ host: "127.0.0.1", port: conn.port });
    let body = "";
    socket.setEncoding("utf8");
    // Large orders import every reference before replying; 10 minutes is generous.
    socket.setTimeout(600000, () => socket.destroy(new CliError("GenDesk 响应超时", 8)));
    socket.on("connect", () =>
      socket.write(`${JSON.stringify({ token: conn.token, cmd, args })}\n`),
    );
    socket.on("data", (chunk) => {
      body += chunk;
    });
    socket.on("error", (e) => reject(e instanceof CliError ? e : offline(e)));
    socket.on("end", () => {
      let reply;
      try {
        reply = JSON.parse(body);
      } catch {
        reject(new CliError("GenDesk 返回了无效响应", 8));
        return;
      }
      if (reply.ok) resolve(reply.data);
      else reject(new CliError(reply.error ?? "GenDesk 处理失败", 5));
    });
  });
}

function offline(error) {
  const e = new CliError(`GenDesk 未运行或控制通道不可达（${error.code ?? error.message}）`, 8);
  e.offline = true;
  return e;
}

/** 应用包路径：GENDESK_APP > 连接文件里的可执行文件所在包 > /Applications/GenDesk.app。 */
function appBundle(conn) {
  if (process.env.GENDESK_APP) return filePath(process.env.GENDESK_APP);
  if (typeof conn?.appPath === "string") {
    const bundle = path.resolve(conn.appPath, "../../..");
    if (path.extname(bundle) === ".app") return bundle;
  }
  return "/Applications/GenDesk.app";
}

/**
 * 连上正在运行的应用；没在运行时（macOS）后台启动并等它写出新的连接文件。
 * `GENDESK_LAUNCH=0` 关闭自动启动（测试与只读诊断用）。
 */
async function connect({ launch }) {
  const conn = await readConnection();
  if (conn) {
    try {
      return { conn, hello: await rpc(conn, "hello") };
    } catch (e) {
      if (!e.offline) throw e;
    }
  }
  check(launch && process.env.GENDESK_LAUNCH !== "0", "GenDesk 未运行。请先打开 GenDesk 再重试", 8);
  check(process.platform === "darwin", "GenDesk 未运行；自动后台启动仅支持 macOS", 8);
  const bundle = appBundle(conn);
  const started = spawnSync("/usr/bin/open", ["-g", "-a", bundle, "--args", "--background"], {
    encoding: "utf8",
  });
  check(
    !started.error && started.status === 0,
    `后台启动 GenDesk 失败（${bundle}）：${started.error?.message ?? started.stderr}`,
    8,
  );
  const deadline = Date.now() + 60000;
  while (Date.now() < deadline) {
    await delay(400);
    const next = await readConnection().catch(() => null);
    if (!next || (conn && next.token === conn.token)) continue;
    try {
      return { conn: next, hello: await rpc(next, "hello"), launched: true };
    } catch (e) {
      if (!e.offline) throw e;
    }
  }
  throw new CliError("已后台启动 GenDesk，但 60 秒内没有连上控制通道；请打开应用查看", 8);
}

// ---------------------------------------------------------------- 预检

async function nativePreflight(conn, prepared) {
  const binary = process.env.GENDESK_APP_BIN ?? conn?.appPath;
  check(
    typeof binary === "string" && binary.trim(),
    "找不到 GenDesk 应用可执行文件做预检：请先启动一次 GenDesk，或用 GENDESK_APP_BIN 指定",
    8,
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
  if (result.error) throw new CliError(`预检调用失败：${result.error.message}`, 8);
  let report;
  try {
    report = JSON.parse(result.stdout);
  } catch {
    throw new CliError(`预检返回无效 JSON：${result.stderr || result.stdout}`);
  }
  check(result.status === 0, report.message ?? "完整预检失败");
  check(
    report.protocolVersion === 1 &&
      report.validation === "full" &&
      report.taskCount === prepared.order.items.length,
    "预检协议或任务数不匹配，未投递",
  );
  return report;
}

// ---------------------------------------------------------------- 投单

const STATUS = { done: "ingested", running: "ingesting" };

function jobResult(jobId, job, extra = {}) {
  if (!job) return { jobId, status: "not_found", ...extra };
  const status = STATUS[job.status] ?? job.status;
  return {
    jobId,
    status,
    job,
    ...extra,
    next:
      status === "hold"
        ? `超过 ${job.taskCount ?? "阈值"} 张的单在等待确认：先向用户说明张数并取得同意，再运行 gendesk confirm ${jobId}`
        : status === "error"
          ? `收录失败：${job.message ?? ""}。修正后运行 gendesk retry ${jobId}`
          : "已建批开跑；生成结果在 GenDesk 审图页人工验收",
  };
}

async function exists(file) {
  return fs
    .stat(file)
    .then(() => true)
    .catch(() => false);
}

/** 已收录的工单目录被移档成 `<jobId>-<ts>`；找出所有同 ID 的包核对内容指纹。 */
async function packagesOf(root, jobId) {
  const found = [];
  const pending = path.join(pendingDir(root), jobId);
  if (await exists(pending)) found.push(pending);
  const entries = await fs.readdir(consumedDir(root)).catch(() => []);
  for (const name of entries)
    if (
      name === jobId ||
      (name.startsWith(`${jobId}-`) && /^\d+$/.test(name.slice(jobId.length + 1)))
    )
      found.push(path.join(consumedDir(root), name));
  return found;
}

async function sameContent(dir, hash) {
  const client = await optionalJson(path.join(dir, "客户端.json")).catch(() => null);
  return client?.contentHash === hash;
}

async function writePackage(root, prepared) {
  const { order, files, refs, hash } = prepared;
  await fs.mkdir(pendingDir(root), { recursive: true });
  const dir = path.join(pendingDir(root), order.jobId);
  try {
    // mkdir without recursive is the inter-process claim for this ID. Never overwrite it.
    await fs.mkdir(dir);
  } catch (e) {
    if (e.code === "EEXIST")
      throw new CliError(`同一 jobId 正在提交：${order.jobId}。请用 status 查询，不会覆盖`);
    throw e;
  }
  // On failure the directory stays WITHOUT READY, so the app refuses to ingest it.
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
  return dir;
}

async function submit(prepared) {
  const { conn, launched } = await connect({ launch: true });
  const { order, hash } = prepared;
  const meta = { taskCount: order.items.length, refCount: prepared.refs.length };
  const packages = await packagesOf(conn.root, order.jobId);
  for (const dir of packages)
    check(
      await sameContent(dir, hash),
      `jobId 已存在且内容不同或不是 CLI 工单：${order.jobId}。请核对，不会覆盖或重投`,
    );
  const known = (await rpc(conn, "intake.status", { jobId: order.jobId })).job;
  const pending = path.join(pendingDir(conn.root), order.jobId);
  if (known && !(await exists(pending)))
    return jobResult(order.jobId, known, { ...meta, reused: true });
  if (await exists(pending)) {
    check(
      await exists(path.join(pending, "READY.txt")),
      `收件目录里有同 ID 的未写完包：${pending}。先检查这份半成品，不要换 ID 重投`,
    );
  } else {
    await nativePreflight(conn, prepared);
    await writePackage(conn.root, prepared);
  }
  const reply = await rpc(conn, "intake.submit", { dir: order.jobId });
  // null = the directory was already ingested by a concurrent call; read the ledger instead.
  const job = reply.job ?? (await rpc(conn, "intake.status", { jobId: order.jobId })).job;
  return jobResult(order.jobId, job, {
    ...meta,
    reused: packages.length > 0,
    ...(launched ? { launched: true } : {}),
  });
}

// ---------------------------------------------------------------- 视频改写

async function rewriteLines(file) {
  const text = await fs.readFile(filePath(file), "utf8").catch((e) => {
    throw new CliError(`无法读取改写结果 ${file}：${e.message}`);
  });
  let lines;
  try {
    const value = JSON.parse(text);
    lines = Array.isArray(value) ? value : value?.lines;
  } catch {
    try {
      lines = text
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean)
        .map((line) => JSON.parse(line));
    } catch {
      throw new CliError("改写结果不是 JSON 数组、{lines} 对象或 JSON Lines");
    }
  }
  check(Array.isArray(lines) && lines.length > 0, "改写结果里没有任何条目");
  for (const [index, line] of lines.entries()) {
    object(line, `第 ${index + 1} 条改写`);
    check(
      line.id !== undefined || line.workId !== undefined || line.clipId !== undefined,
      `第 ${index + 1} 条缺少 id（原样回传 pending 给的 W 编号）`,
    );
  }
  return lines;
}

// ---------------------------------------------------------------- 入口

const COMMANDS = {
  doctor: { positionals: 0, options: [] },
  validate: { positionals: 0, options: ["manifest", "job-id", ...SINGLE_OPTIONS] },
  submit: { positionals: 0, options: ["manifest", "job-id", ...SINGLE_OPTIONS] },
  status: { positionals: 1, options: [] },
  confirm: { positionals: 1, options: [] },
  retry: { positionals: 1, options: [] },
  overview: { positionals: 0, options: [] },
  v2v: { positionals: 1, options: ["file"] },
};

function exitCode(result) {
  return { hold: 4, error: 5, duplicate: 6, not_found: 7 }[result.status] ?? 0;
}

async function main() {
  const { values: opts, positionals } = parseArgs({
    allowPositionals: true,
    options: Object.fromEntries([
      ...["manifest", "job-id", "file", ...SINGLE_OPTIONS].map((key) => [key, { type: "string" }]),
      ...["help", "version"].map((key) => [key, { type: "boolean" }]),
    ]),
  });
  if (opts.version) {
    check(!positionals.length && Object.keys(opts).length === 1, "--version 不能与其他参数混用");
    process.stdout.write(
      `${JSON.stringify({ cliVersion: CLI_VERSION, protocol: PROTOCOL, orderVersion: ORDER_VERSION }, null, 2)}\n`,
    );
    return;
  }
  if (opts.help || !positionals.length) {
    process.stdout.write(`${HELP}\n`);
    return;
  }
  const [command, ...rest] = positionals;
  const spec = COMMANDS[command];
  check(spec, `未知命令：${command}；请运行 gendesk --help`);
  check(rest.length === spec.positionals, "命令位置参数不正确；请运行 gendesk --help");
  for (const key of Object.keys(opts))
    check(spec.options.includes(key), `${command} 不支持 --${key}`);

  let result;
  switch (command) {
    case "doctor": {
      const conn = await readConnection().catch((e) => ({ error: e.message }));
      let app = null;
      let message = "GenDesk 在线，可以投单";
      if (conn && !conn.error) {
        try {
          app = await rpc(conn, "hello");
        } catch (e) {
          message = `${e.message}；submit 会在后台启动它`;
        }
      } else message = conn?.error ?? "还没有连接文件：GenDesk 从未启动过，submit 会在后台启动它";
      result = {
        status: app ? "online" : "offline",
        cliVersion: CLI_VERSION,
        protocol: PROTOCOL,
        connection: CONNECTION,
        root: app?.root ?? ROOT,
        app,
        nodeVersion: process.version,
        message,
        review: "生图结果进入 GenDesk 审图页人工验收",
      };
      break;
    }
    case "validate": {
      const prepared = await prepare(opts);
      const conn = await readConnection().catch(() => null);
      const preflight = await nativePreflight(conn, prepared);
      result = {
        status: "validated",
        validation: "full",
        jobId: prepared.order.jobId,
        taskCount: prepared.order.items.length,
        refCount: prepared.refs.length,
        order: prepared.order,
        preflight,
        message: "完整预检通过（参数、归属、正文与实际图片解码）；未投递、未调用生图",
      };
      break;
    }
    case "submit":
      result = await submit(await prepare(opts));
      break;
    case "status":
    case "confirm":
    case "retry": {
      const jobId = requiredText(rest[0], "jobId");
      const { conn } = await connect({ launch: command !== "status" });
      const reply = await rpc(conn, `intake.${command}`, { jobId });
      result = jobResult(jobId, reply.job);
      break;
    }
    case "overview": {
      const { conn } = await connect({ launch: false });
      result = { status: "ok", ...(await rpc(conn, "overview")) };
      break;
    }
    case "v2v": {
      const [sub] = rest;
      check(sub === "pending" || sub === "rewrite", "v2v 只支持 pending / rewrite");
      if (sub === "pending") {
        check(opts.file === undefined, "v2v pending 不接受 --file");
        const { conn } = await connect({ launch: true });
        result = { status: "ok", ...(await rpc(conn, "v2v.pending")) };
      } else {
        const lines = await rewriteLines(requiredText(opts.file, "--file"));
        const { conn } = await connect({ launch: true });
        result = { status: "ok", ...(await rpc(conn, "v2v.rewrite", { lines })) };
      }
      break;
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
