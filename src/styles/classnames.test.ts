import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * 每个 `className` 用到的类名都必须在 `globals.css` 里真的存在。
 *
 * ## 为什么需要这条测试
 *
 * v0.19.0–v0.21.0 的视频流水线三个页面把滚动容器写成 `className="sc"`，
 * 而 **`.sc` 从来没有定义过** —— 它只存在于 `docs/prototype/prototype.dc.html`。
 * 于是那个 div 只有 `flex:1`、没有 `overflow`，内容溢出后被上游
 * `.ohide` / `.main{overflow:hidden}` 裁掉：没有滚动条、没有滚轮目标，
 * 第一屏之后的行**物理上够不着**。整整两个版本没人发现。
 *
 * 而既有测试一条都抓不到它：它们全在派生逻辑上（`model.ts` / `layout.ts`），
 * 而一个不存在的 class 不会报错、不会红、类型检查也管不着 —— 它只是**静默地
 * 什么都不做**。这一类缺陷（拼错、改名后漏改、从原型抄了个没实现的类）只能靠
 * 「用到的都得存在」这条机械规则挡住。
 *
 * ## 为什么 Tailwind 的工具类也算违规
 *
 * `globals.css` 第一行是 `@import "tailwindcss"`，所以 `gap-2` 这类工具类**确实**
 * 会生效。但本仓库不用它们（`fx` / `ac` / `mt5` 全是自定义），铁律 5 也要求
 * token 只从 `globals.css` 取。这里把「不用 Tailwind」写成机制：写了 `gap-2` 的人
 * 会看到一条说明了理由的红，而不是一个两种体系混用、谁也说不清该改哪边的代码库。
 *
 * ## 三条规则，同一个道理
 *
 * 1. **用到的 class 必须存在** —— 就是 `.sc` 那一课。
 * 2. **引用的 CSS 变量必须存在**：`var(--card)` 没定义时，`background` 整条声明
 *    直接作废（写了带 fallback 的 `var(--card, #fff)` 更糟：它会一直用兜底色，
 *    换主题时那几块永远是白的）。查出来的三处正是这样：`--card` / `--bg1` / `--sans`
 *    从来没有定义过，而对应的规则在界面上就是「看着好像也没问题」。
 * 3. **定义了的 class 必须有人用**：反过来那一半。删一个组件很容易忘了删它的样式，
 *    于是 globals.css 只增不减 —— 本次一次清掉 40 个死类、327 行。
 *
 * ## 这条测试**不**覆盖什么
 *
 * 由辅助函数返回的类名（`statusVisual().badgeClass`、`toneClass()`）不出现在
 * `className=` 位置，正向检查扫不到。那是有意接受的缺口：`.sc` 是字面量，
 * 当初查出的 11 处违规也全是字面量 —— 先把这一类堵死，比追求完备更要紧。
 * 反向检查为此用的是**宽判据**（整个 src 里出现过这个词就算用过），
 * 宁可漏报也不能误删一条正在生效的样式。
 */

// vitest 从仓库根跑（vite.config.ts 在那儿）。不用 import.meta.url —— vite 会把它
// 改写成 `/@fs/...` 形式的 URL，readFileSync 拿它当路径会 ENOENT。
const ROOT = process.cwd();

/**
 * 允许清单。每一项都必须写清**为什么它不在 globals.css 里**，
 * 否则它就是下一个 `.sc`。
 */
const ALLOW = new Set([
  // AppShell 按平台加 `mac`/`win`：`.app.win` 有规则（Windows 窗控留位），
  // `.app.mac` 是故意留空的钩子 —— mac 上不需要任何额外样式。
  "mac",
  // PlanPage 的行只需要 cursor:pointer，就地写在 style 里；这个词只作语义标记。
  "clickable",
  // GeneratePage 用它把某个输入框的边框还原成无边框，实际样式写在 style 里。
  "unset",
]);

/**
 * 由 JSX 内联 `style` 下发、而不是在 globals.css 里声明的自定义属性。
 * 每一项都要写清**谁在设它**。
 */
const ALLOW_TOKENS = new Set([
  // 验收页齐行排版：每一行的行高由 layout.ts 算出来，逐行经 style 下发
  // （ReviewPage 的 `style={{ "--rjh": `${row.h}px` }}`）。它按定义就不该有全局默认值。
  "--rjh",
]);

/**
 * 定义了但暂时没人用的 class。**空清单是目标状态** —— 每加一项都是欠一笔债。
 */
const ALLOW_UNUSED = new Set<string>([]);

/** 收集 globals.css 里定义过的所有类名。 */
function definedClasses(css: string): Set<string> {
  // 注释先整段去掉：里面遍地是 `prototype.dc.html`、`.vrowh` 这类**在讲**类名的散文，
  // 留着它们会让「定义过的类」凭空多出一批，反向检查就再也报不出真正的死类。
  // 再把声明块整个抹掉，否则属性值里的 `.5rem`、`content: ".x"` 会被当成类名。
  const selectorsOnly = css.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\{[^{}]*\}/g, "{}");
  const out = new Set<string>();
  for (const m of selectorsOnly.matchAll(/\.(-?[A-Za-z_][\w-]*)/g)) {
    if (m[1]) out.add(m[1]);
  }
  return out;
}

/** 递归收集 src 下的 .tsx。 */
function tsxFiles(dir: string, acc: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) tsxFiles(p, acc);
    else if (e.name.endsWith(".tsx")) acc.push(p);
  }
  return acc;
}

/** 递归收集 src 下的 .tsx / .ts（测试自身除外）。 */
function sourceFiles(dir: string, acc: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) sourceFiles(p, acc);
    else if ((e.name.endsWith(".tsx") || e.name.endsWith(".ts")) && !e.name.endsWith(".test.ts")) {
      acc.push(p);
    }
  }
  return acc;
}

/** `--foo: …` 声明过的自定义属性。 */
function definedTokens(css: string): Set<string> {
  return new Set(Array.from(css.matchAll(/(--[A-Za-z0-9_-]+)\s*:/g), (m) => m[1] ?? ""));
}

/** `var(--foo …)` 引用到的自定义属性，带出处。 */
function tokenRefs(src: string, rel: string): { token: string; where: string }[] {
  return Array.from(src.matchAll(/var\(\s*(--[A-Za-z0-9_-]+)/g), (m) => ({
    token: m[1] ?? "",
    where: `${rel}:${src.slice(0, m.index ?? 0).split("\n").length}`,
  }));
}

/**
 * 从一段 `className=` 的值里取出**处于类名位置**的字符串字面量。
 *
 * 表达式里还有别的字符串，它们不是类名，混进来会造成一堆假阳性：
 * `cn("btn", stage === "pass" && "on")` 里的 `"pass"` 是比较操作数，
 * `x.includes("vip")` 里的是方法参数。逐条剔掉，剩下的才是类名。
 */
function classLiterals(expr: string): string[] {
  const cleaned = expr
    // 比较操作数：x === "pass" / "pass" === x
    .replace(/[=!]==?\s*"[^"]*"/g, "")
    .replace(/"[^"]*"\s*[=!]==?/g, "")
    // 空值合并的右手边：(s.motion ?? "standard")
    .replace(/\?\?\s*"[^"]*"/g, "")
    // 方法参数：.includes("vip")
    .replace(/\.\w+\(\s*"[^"]*"\s*\)/g, "")
    // 对象键：{ "pass": … }。**必须要求前面是 `{` 或 `,`**，否则会把三元的
    // `cond ? "a" : "b"` 里的 `"a" :` 一起吃掉 —— 那会静默漏掉一个真类名，
    // 比多报一个假阳性糟得多。
    .replace(/([{,])\s*"[^"]*"\s*:/g, "$1")
    // 索引：map["pass"]
    .replace(/\[\s*"[^"]*"\s*\]/g, "");
  const out: string[] = [];
  for (const m of cleaned.matchAll(/"([^"]*)"/g)) {
    for (const tok of (m[1] ?? "").split(/\s+/)) {
      if (tok !== "") out.push(tok);
    }
  }
  return out;
}

/** 取 `className=` 后面那一整段（引号字面量，或大括号表达式——按配对计数）。 */
function classNameExprs(src: string): { expr: string; line: number }[] {
  const out: { expr: string; line: number }[] = [];
  for (const m of src.matchAll(/className=/g)) {
    const at = (m.index ?? 0) + "className=".length;
    const line = src.slice(0, at).split("\n").length;
    if (src[at] === '"') {
      const end = src.indexOf('"', at + 1);
      if (end > at) out.push({ expr: src.slice(at, end + 1), line });
      continue;
    }
    if (src[at] !== "{") continue;
    let depth = 0;
    for (let i = at; i < src.length; i += 1) {
      if (src[i] === "{") depth += 1;
      else if (src[i] === "}") {
        depth -= 1;
        if (depth === 0) {
          out.push({ expr: src.slice(at, i + 1), line });
          break;
        }
      }
    }
  }
  return out;
}

describe("className 与 globals.css 必须对得上", () => {
  it("每个用到的类名都在 globals.css 里有定义", () => {
    const defined = definedClasses(readFileSync(join(ROOT, "src/styles/globals.css"), "utf8"));
    const bad: string[] = [];

    for (const file of tsxFiles(join(ROOT, "src"))) {
      const src = readFileSync(file, "utf8");
      const rel = file.slice(ROOT.length);
      for (const { expr, line } of classNameExprs(src)) {
        for (const tok of classLiterals(expr)) {
          if (defined.has(tok) || ALLOW.has(tok)) continue;
          bad.push(`${tok} → ${rel}:${line}`);
        }
      }
    }

    expect(
      bad,
      `这些 class 在 globals.css 里不存在，写了等于没写（.sc 就是这么让三个页面两个版本不能滚动的）。\n补上规则，或——若它确实有意为之——加进本文件的 ALLOW 并写明理由：\n  ${bad.join("\n  ")}`,
    ).toEqual([]);
  });

  it("解析器不把比较操作数当成类名 —— 否则它会被一堆假阳性淹掉而没人再看", () => {
    // 这几种写法在本仓库里遍地都是；把它们当类名会让这条测试从第一天起就是红的。
    expect(classLiterals('{cn("btn", stage === "pass" && "on")}')).toEqual(["btn", "on"]);
    expect(classLiterals('{cn("vchip", s.includes("vip") && "on")}')).toEqual(["vchip", "on"]);
    expect(classLiterals('{(m ?? "standard") === "standard" ? "a" : "b"}')).toEqual(["a", "b"]);
    // 多个类写在一个字面量里要拆开。
    expect(classLiterals('"col f1 ohide"')).toEqual(["col", "f1", "ohide"]);
  });

  it("每个 var(--x) 引用的 token 都在 globals.css 里有定义", () => {
    const css = readFileSync(join(ROOT, "src/styles/globals.css"), "utf8");
    const defined = definedTokens(css);
    const bad: string[] = [];

    for (const file of [...sourceFiles(join(ROOT, "src")), join(ROOT, "src/styles/globals.css")]) {
      const src = readFileSync(file, "utf8");
      const rel = file.slice(ROOT.length);
      for (const { token, where } of tokenRefs(src, rel)) {
        if (defined.has(token) || ALLOW_TOKENS.has(token)) continue;
        bad.push(`${token} → ${where}`);
      }
    }

    expect(
      bad,
      `这些 CSS 变量没有定义。没有 fallback 时整条声明作废；有 fallback（var(--card, #fff)）\n更隐蔽——它会一直用兜底值，换主题时那几块永远不跟：\n  ${bad.join("\n  ")}`,
    ).toEqual([]);
  });

  it("globals.css 里定义的 class 都得有人用", () => {
    const css = readFileSync(join(ROOT, "src/styles/globals.css"), "utf8");
    const defined = definedClasses(css);
    const all = sourceFiles(join(ROOT, "src"))
      .map((f) => readFileSync(f, "utf8"))
      .join("\n");

    // **宽判据**：整个 src 里出现过这个词就算用过（含辅助函数拼出来的类名、
    // 模板字符串里的）。宁可漏报也不能误删一条正在生效的样式。
    const dead = [...defined]
      .filter((c) => !ALLOW_UNUSED.has(c))
      .filter((c) => !new RegExp(`["\\s\`.]${c.replace(/[-]/g, "\\-")}["\\s\`]`).test(all));

    expect(
      dead,
      `这些 class 在 globals.css 里有规则，却没有任何地方用到 —— 删组件时忘了删样式，\n于是 globals.css 只增不减。删掉它们，或加进 ALLOW_UNUSED 并写明理由：\n  ${dead.join(" ")}`,
    ).toEqual([]);
  });

  it("确实抓得住「用了但没定义」—— 否则这条测试自己就是个摆设", () => {
    const defined = definedClasses(".vtbody { overflow: auto } .f1 { flex: 1 }");
    expect(defined.has("vtbody")).toBe(true);
    // 就是当年那一行：`.sc` 没定义，而它同一行的 `f1` 定义了 —— 只有前者该被报出来。
    const used = classLiterals('{"sc f1"}');
    expect(used.filter((t) => !defined.has(t))).toEqual(["sc"]);

    // 声明块要被抹掉：属性值里的 `.5rem` 不是类名。
    expect(definedClasses(".a { margin: .5rem }").has("5rem")).toBe(false);
  });
});
