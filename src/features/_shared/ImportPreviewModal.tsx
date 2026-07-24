import { Modal } from "@/components/ui/Modal";
import {
  type ImportPreview,
  type ImportPreviewGroup,
  type ImportPreviewPrompt,
  commands,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import {
  AlertTriangle,
  ArrowDownToLine,
  ArrowUpToLine,
  Check,
  ChevronDown,
  ChevronRight,
  FileDown,
  HelpCircle,
  Scissors,
  Trash2,
} from "lucide-react";
import { useCallback, useRef, useState } from "react";
import { toast } from "sonner";

/**
 * 提示词 .txt 导入预览确认弹窗（E37 格式帮助 + 行号级报错；E14 生成页/库共用）。
 *
 * 解析器给出的是**初稿**，这里可以直接改：改组名/前缀、确认或否掉「疑似」分组、
 * 上下并组、按条拆组、改正文小标题、删条删组。改完点导入即按屏幕上所见落库——
 * 不需要回去改 txt 再导一遍。结构性改动会调 `repreviewImport` 重算前缀与编号区间。
 *
 * 父组件在 onConfirm 收到的是**编辑后**的 preview（区分 library / generate 上下文提交）。
 */
export function ImportPreviewModal({
  preview,
  note,
  confirmLabel,
  onConfirm,
  onClose,
}: {
  preview: ImportPreview;
  /** 上下文说明（如生成页「作为本批次临时分组」）。 */
  note?: string;
  /** 确认按钮动词，条数由本组件按编辑后的实际条数补上。 */
  confirmLabel: string;
  onConfirm: (edited: ImportPreview) => void;
  onClose: () => void;
}) {
  const [showHelp, setShowHelp] = useState(false);
  const [draft, setDraft] = useState<ImportPreview>(preview);
  const [expanded, setExpanded] = useState<number | null>(null);
  const seq = useRef(0);

  const total = draft.groups.reduce((n, g) => n + g.prompts.length, 0);

  /** 纯前端改动（改字），不动结构。 */
  const patch = (next: ImportPreview) => setDraft(next);

  /** 结构性改动：本地先生效，再让后端重算前缀 / 编号区间 / 是否新建组。 */
  const restructure = useCallback(async (next: ImportPreview) => {
    setDraft(next);
    const id = ++seq.current;
    const res = await unwrap(commands.repreviewImport(next)).catch((e) => {
      toast.error(String(e));
      return null;
    });
    // 只认最后一次请求的结果，避免连点后回填旧结构。
    if (res && id === seq.current) setDraft(res);
  }, []);

  const mapGroups = (fn: (gs: ImportPreviewGroup[]) => ImportPreviewGroup[]) => ({
    ...draft,
    groups: fn(draft.groups),
  });
  const withGroup = (i: number, up: (g: ImportPreviewGroup) => ImportPreviewGroup) =>
    mapGroups((gs) => gs.map((g, k) => (k === i ? up(g) : g)));

  /** 与相邻组合并（dir=-1 上 / +1 下）：正文按顺序接到目标组尾，本组消失。 */
  const merge = (i: number, dir: -1 | 1) => {
    const j = i + dir;
    // 本组（i）并进邻组（j），正文按文档原顺序拼接；邻组的组名/前缀保留。
    const src = draft.groups[i];
    const dst = draft.groups[j];
    if (!src || !dst) return;
    const prompts =
      dir === -1 ? [...dst.prompts, ...src.prompts] : [...src.prompts, ...dst.prompts];
    void restructure(
      mapGroups((gs) =>
        gs.map((g, k) => (k === j ? { ...g, prompts } : g)).filter((_, k) => k !== i),
      ),
    );
    setExpanded(null);
  };

  /** 从第 j 条起拆成新分组（新组名先沿用原名加序号，可当场改）。 */
  const splitAt = (i: number, j: number) => {
    const g = draft.groups[i];
    if (!g || j <= 0 || j >= g.prompts.length) return;
    const head: ImportPreviewGroup = { ...g, prompts: g.prompts.slice(0, j) };
    const tail: ImportPreviewGroup = {
      ...g,
      name: `${g.name} 2`,
      prefix: "",
      prefixExplicit: false,
      inferred: false,
      prompts: g.prompts.slice(j),
    };
    void restructure(mapGroups((gs) => [...gs.slice(0, i), head, tail, ...gs.slice(i + 1)]));
  };

  const removeGroup = (i: number) => {
    void restructure(mapGroups((gs) => gs.filter((_, k) => k !== i)));
    setExpanded(null);
  };

  const removePrompt = (i: number, j: number) => {
    void restructure(withGroup(i, (g) => ({ ...g, prompts: g.prompts.filter((_, k) => k !== j) })));
  };

  const setPrompt = (i: number, j: number, up: Partial<ImportPreviewPrompt>) =>
    patch(
      withGroup(i, (g) => ({
        ...g,
        prompts: g.prompts.map((p, k) => (k === j ? { ...p, ...up } : p)),
      })),
    );

  const saveTemplate = async () => {
    const path = await unwrap(commands.savePromptTemplate()).catch((e) => {
      toast.error(String(e));
      return null;
    });
    if (path) toast.success("已保存模板 txt");
  };

  const suspects = draft.groups.filter((g) => g.inferred).length;

  return (
    <Modal
      title="导入提示词 .txt"
      width="w700"
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn sm gho" onClick={saveTemplate}>
            <FileDown className="ic12" />
            保存模板
          </button>
          <div className="f1" />
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="btn pri"
            disabled={total === 0}
            onClick={() => onConfirm({ ...draft, total })}
          >
            {confirmLabel} {total} 条
          </button>
        </>
      }
    >
      <div className="fx ac gap8">
        <span className="chip">{draft.encoding}</span>
        <span className="fs11 t3">
          解析出 {draft.groups.length} 组 · {total} 条
        </span>
        {note && <span className="bdg b-amber">{note}</span>}
        <div className="f1" />
        <button
          type="button"
          className="btn sm gho"
          onClick={() => setShowHelp((v) => !v)}
          title="txt 格式说明"
        >
          <HelpCircle className="ic12" />
          格式说明
        </button>
      </div>

      {showHelp && (
        <pre className="impfmt">{`分组不用写成固定格式，认不准的地方直接在下面这张表里改就行。

它会自己认这些写法：
【分组名称】/ 分组: 名称 / 分组【名称】/ 组-名称   ← 显式写法，优先听你的
一行短标题 + 下面一堆长段落                        ← 没写标记时按形态推断，标「疑似」
一行短标题 + 下面只有一条长段落                    ← 认作那条正文的小标题
什么线索都没有                                      ← 用文件名当分组名

可选的元信息行：前缀: DZ / 场景: 商品 / 标签: 白底, 主图
正文一行一条，空行忽略；前导序号 1. 2、 3）(4)（5)① 会自动去掉。`}</pre>
      )}

      {suspects > 0 && (
        <div className="impwarn">
          <div className="fx ac gap6 fw6 fs12">
            <AlertTriangle className="ic12" />
            {suspects} 个分组是按文档结构猜的
          </div>
          <div className="fs11 t2 impwarn-item">
            文件里没写分组标记，已按「短标题行 + 其下多条正文」推断。对就点「就这样」，
            不对就直接改名、并入上一/下一组，或删掉。
          </div>
        </div>
      )}

      {draft.warnings.length > 0 && (
        <div className="impwarn">
          <div className="fx ac gap6 fw6 fs12">
            <AlertTriangle className="ic12" />
            {draft.warnings.length} 处格式提示（不影响导入）
          </div>
          {draft.warnings.map((w) => (
            <div key={`${w.line}-${w.message}`} className="fs11 t2 impwarn-item">
              {w.line > 0 && `第 ${w.line} 行：`}
              {w.message}
            </div>
          ))}
        </div>
      )}

      <div className="impgrps">
        {draft.groups.map((g, i) => (
          <div key={`${i}-${g.prefix}`} className={cn("impgrp", g.inferred && "sus")}>
            <div className="impgrp-head">
              <button
                type="button"
                className="icb"
                onClick={() => setExpanded((v) => (v === i ? null : i))}
                title={expanded === i ? "收起条目" : "展开逐条编辑"}
              >
                {expanded === i ? (
                  <ChevronDown className="ic12" />
                ) : (
                  <ChevronRight className="ic12" />
                )}
              </button>
              <input
                className="inp sm impname"
                value={g.name}
                placeholder="分组名"
                aria-label="分组名"
                onChange={(e) => patch(withGroup(i, (x) => ({ ...x, name: e.target.value })))}
                onBlur={() => void restructure(draft)}
              />
              <input
                className="inp sm impprefix"
                value={g.prefix}
                placeholder="前缀"
                aria-label="编号前缀"
                title="编号前缀（编号形如 DZ-0001）"
                onChange={(e) =>
                  patch(
                    withGroup(i, (x) => ({
                      ...x,
                      prefix: e.target.value.toUpperCase(),
                      prefixExplicit: true,
                    })),
                  )
                }
                onBlur={() => void restructure(draft)}
              />
              {g.inferred ? (
                <button
                  type="button"
                  className="bdg b-amber impsus"
                  title="确认这个分组名，去掉「疑似」标记"
                  onClick={() => patch(withGroup(i, (x) => ({ ...x, inferred: false })))}
                >
                  <Check className="ic12" />
                  疑似 · 就这样
                </button>
              ) : (
                g.isNewGroup === false && <span className="bdg b-gray">并入已有组</span>
              )}
              <div className="f1" />
              <span className="chip nowrap">{g.codeRange}</span>
              <span className="t3 fs11 nowrap">{g.prompts.length} 条</span>
              <button
                type="button"
                className="icb"
                disabled={i === 0}
                title="并入上一组"
                onClick={() => merge(i, -1)}
              >
                <ArrowUpToLine className="ic12" />
              </button>
              <button
                type="button"
                className="icb"
                disabled={i === draft.groups.length - 1}
                title="并入下一组"
                onClick={() => merge(i, 1)}
              >
                <ArrowDownToLine className="ic12" />
              </button>
              <button
                type="button"
                className="icb impdel"
                title={`不导入这组（${g.prompts.length} 条）`}
                onClick={() => removeGroup(i)}
              >
                <Trash2 className="ic12" />
              </button>
            </div>

            {expanded === i && (
              <div className="impgrp-body">
                {g.prompts.map((p, j) => (
                  <div key={`${j}-${p.text.slice(0, 12)}`} className="improw">
                    <span className="improw-n">{j + 1}</span>
                    <div className="f1 fx col gap6">
                      <input
                        className="inp sm"
                        value={p.title ?? ""}
                        placeholder="小标题（可选）"
                        aria-label="小标题"
                        onChange={(e) => setPrompt(i, j, { title: e.target.value || null })}
                      />
                      <textarea
                        className="ta impbody"
                        value={p.text}
                        aria-label="提示词正文"
                        onChange={(e) => setPrompt(i, j, { text: e.target.value })}
                      />
                    </div>
                    <div className="fx col gap4">
                      <button
                        type="button"
                        className="icb"
                        disabled={j === 0}
                        title="从这条起拆成新分组"
                        onClick={() => splitAt(i, j)}
                      >
                        <Scissors className="ic12" />
                      </button>
                      <button
                        type="button"
                        className="icb impdel"
                        title="不导入这条"
                        onClick={() => removePrompt(i, j)}
                      >
                        <Trash2 className="ic12" />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}
      </div>
    </Modal>
  );
}
