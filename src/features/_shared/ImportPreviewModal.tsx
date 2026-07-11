import { Modal } from "@/components/ui/Modal";
import { type ImportPreview, commands, unwrap } from "@/lib/ipc";
import { AlertTriangle, FileDown, HelpCircle } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

/**
 * 提示词 .txt 导入预览确认弹窗（E37 格式帮助 + 行号级报错；E14 生成页/库共用）。
 * 父组件负责在 onConfirm 内 commit（区分 library / generate 上下文）。
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
  confirmLabel: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const [showHelp, setShowHelp] = useState(false);
  const warnings = preview.warnings;

  const saveTemplate = async () => {
    const path = await unwrap(commands.savePromptTemplate()).catch((e) => {
      toast.error(String(e));
      return null;
    });
    if (path) toast.success("已保存模板 txt");
  };

  return (
    <Modal
      title="导入提示词 .txt"
      width="w640"
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
          <button type="button" className="btn pri" onClick={onConfirm}>
            {confirmLabel}
          </button>
        </>
      }
    >
      <div className="fx ac gap8">
        <span className="chip">{preview.encoding}</span>
        <span className="fs11 t3">
          解析出 {preview.groups.length} 组 · {preview.total} 条
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
        <pre className="impfmt">{`【分组名称】        ← 独占一行的括号即分组；也支持 分组: 名称 / 分组【名称】
前缀: DZ            ← 可选，缺省自动生成；编号形如 DZ-0001
场景: 商品          ← 可选
标签: 白底, 主图    ← 可选，逗号/顿号/空格分隔

【小标题】          ← 可选，紧贴其下方那条正文即视为它的小标题
1. 这是一条提示词正文（前导序号会自动去除）
另一条正文，一行一条，空行忽略

· 括号可用 【】/[]/［］/〖〗；序号支持 1. 2、 3）(4)（5）① 等
· 括号行的下一行是正文→当小标题，否则→当分组，无需写「分组:」`}</pre>
      )}

      {warnings.length > 0 && (
        <div className="impwarn">
          <div className="fx ac gap6 fw6 fs12">
            <AlertTriangle className="ic12" />
            {warnings.length} 处格式提示（不影响导入）
          </div>
          {warnings.map((w) => (
            <div key={`${w.line}-${w.message}`} className="fs11 t2 impwarn-item">
              第 {w.line} 行：{w.message}
            </div>
          ))}
        </div>
      )}

      <div
        className="mt10"
        style={{ border: "1px solid var(--line)", borderRadius: 9, overflow: "hidden" }}
      >
        {preview.groups.map((o) => (
          <div key={o.prefix} className="fx ac gap9" style={{ padding: "9px 11px" }}>
            <i className="gdot" style={{ background: "var(--wr)" }} />
            <span className="fw5 fs12 nowrap">{o.name}</span>
            <span className="chip">{o.prefix}</span>
            <span className="t3 fs11 f1 nowrap ohide">{o.tags.join(" · ")}</span>
            <span className="chip">{o.codeRange}</span>
            <span className="t3 fs11 nowrap">{o.count} 条</span>
          </div>
        ))}
      </div>
    </Modal>
  );
}
