import { Modal } from "@/components/ui/Modal";
import { copyText } from "@/features/_shared/MediaActions";
import {
  type LibraryCategoryView,
  type LibraryImageView,
  type LibraryImportPreviewEntry,
  type LibraryRelinkEntry,
  type LibraryStyleView,
  type LibraryUsageView,
  commands,
  unwrap,
} from "@/lib/ipc";
import { useEffect, useState } from "react";
import { toast } from "sonner";

const importActions: Record<string, string> = {
  new: "新增",
  reuse: "复用记录",
  duplicate: "重复",
  invalid: "无法读取",
};

export function LibraryImportDialog({
  paths,
  style,
  categories,
  initialCategory,
  onClose,
  onImport,
}: {
  paths: string[];
  style: LibraryStyleView;
  categories: LibraryCategoryView[];
  initialCategory: number | null;
  onClose: () => void;
  onImport: (paths: string[], categoryId: number) => void;
}) {
  const [category, setCategory] = useState(initialCategory);
  const [rows, setRows] = useState<LibraryImportPreviewEntry[] | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt is the explicit retry trigger.
  useEffect(() => {
    let current = true;
    void unwrap(commands.previewLibraryImport(style.id, paths))
      .then((value) => {
        if (current) setRows(value);
      })
      .catch((error) => {
        if (current) setError(String(error));
      });
    return () => {
      current = false;
    };
    // The retry button re-runs preflight after files become available again.
  }, [style.id, paths, attempt]);
  const ready = rows?.filter((row) => row.action === "new" || row.action === "reuse") ?? [];
  return (
    <Modal
      title={`导入预览 · ${style.name}`}
      width="w700"
      onClose={onClose}
      footer={
        <button
          type="button"
          className="btn sm pri"
          disabled={!category || ready.length === 0}
          onClick={() =>
            category &&
            onImport(
              ready.map((row) => row.path),
              category,
            )
          }
        >
          导入 {ready.length} 张
        </button>
      }
    >
      <label className="fx ac gap8 fs12">
        目标分类
        <select
          className="inp sm"
          value={category ?? ""}
          onChange={(event) => setCategory(Number(event.target.value) || null)}
        >
          <option value="">选择分类…</option>
          {categories.map((item) => (
            <option key={item.id} value={item.id}>
              {item.name}
            </option>
          ))}
        </select>
      </label>
      {error ? (
        <p className="fs12">
          {error}{" "}
          <button
            type="button"
            className="btn xs"
            onClick={() => {
              setError("");
              setAttempt((value) => value + 1);
            }}
          >
            重新检查
          </button>
        </p>
      ) : !rows ? (
        <p className="fs12 t3" role="status">
          正在检查 {paths.length} 个文件…
        </p>
      ) : (
        <>
          <p className="fs12 t2">
            {Object.entries(importActions)
              .map(
                ([action, label]) =>
                  `${label} ${rows.filter((row) => row.action === action).length}`,
              )
              .join(" · ")}
          </p>
          <div style={{ maxHeight: "42vh", overflow: "auto" }}>
            {rows.map((row, index) => (
              <p key={`${row.path}-${index}`} className="fs11 mt10">
                <strong>{importActions[row.action]}</strong> · {row.path}
                {row.detail && ` · ${row.detail}`}
              </p>
            ))}
          </div>
        </>
      )}
    </Modal>
  );
}

export function LibraryUsageDialog({
  image,
  onClose,
  onSaved,
}: {
  image: LibraryImageView;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [note, setNote] = useState(image.note);
  const [uses, setUses] = useState<LibraryUsageView[] | null>(null);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const load = async () => {
    setError("");
    try {
      setUses(await unwrap(commands.libraryImageUsage(image.id)));
    } catch (error) {
      setError(String(error));
    }
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: the dialog is mounted for one image.
  useEffect(() => {
    void load();
  }, []);
  const save = async () => {
    setSaving(true);
    try {
      await unwrap(
        commands.updateLibraryImages([image.id], {
          categoryId: null,
          enabled: null,
          removed: null,
          note,
        }),
      );
      await onSaved();
      toast.success("参考图备注已保存");
      onClose();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSaving(false);
    }
  };
  return (
    <Modal
      title={`#${image.id} · ${image.name}`}
      width="w700"
      onClose={onClose}
      footer={
        <>
          <button
            type="button"
            className="btn sm"
            onClick={() => void copyText(String(image.id), "素材编号")}
          >
            复制素材编号
          </button>
          <button
            type="button"
            className="btn sm pri"
            disabled={saving || note === image.note}
            onClick={() => void save()}
          >
            {saving ? "正在保存…" : "保存备注"}
          </button>
        </>
      }
    >
      <p className="fs12 t2">
        {image.style} · {image.category} · 使用 {image.useCount} 次
        {image.coolingUntil && ` · 冷却至 ${new Date(image.coolingUntil * 1000).toLocaleString()}`}
      </p>
      <label className="fs12">
        备注
        <textarea
          className="inp"
          style={{ display: "block", width: "100%" }}
          value={note}
          onChange={(event) => setNote(event.target.value)}
        />
      </label>
      <p className="fs12 t2">工单优先抽取可用图片。候选不足时按较短冷却期回退。</p>
      {error ? (
        <p className="fs12">
          {error}{" "}
          <button className="btn xs" type="button" onClick={() => void load()}>
            重读用量
          </button>
        </p>
      ) : uses === null ? (
        <p className="fs12 t3" role="status">
          正在读取用量…
        </p>
      ) : uses.length === 0 ? (
        <p className="fs12 t3">暂无使用记录</p>
      ) : (
        <ul style={{ maxHeight: "34vh", overflow: "auto" }}>
          {uses.map((entry, index) => (
            <li className="fs11 mt10" key={`${entry.usedAt}-${index}`}>
              {new Date(entry.usedAt * 1000).toLocaleString()} · {entry.itemName ?? entry.source}
              {entry.slot !== null && ` · 槽位 ${entry.slot}`}
              {entry.fallback && ` · 冷却回退 ${entry.cooldownDays ?? 0} 天`}
              {entry.jobId && (
                <button
                  type="button"
                  className="btn xs"
                  onClick={() => entry.jobId && void copyText(entry.jobId, "工单编号")}
                >
                  {entry.jobId}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </Modal>
  );
}

export function RelinkDirectoryDialog({
  images,
  onClose,
  onApplied,
}: {
  images: LibraryImageView[];
  onClose: () => void;
  onApplied: () => Promise<void>;
}) {
  const [original, setOriginal] = useState(images[0]?.filePath.replace(/[/\\][^/\\]+$/, "") ?? "");
  const [replacement, setReplacement] = useState("");
  const [entries, setEntries] = useState<LibraryRelinkEntry[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [applied, setApplied] = useState(false);
  const matched = entries?.filter((entry) => entry.newPath).length ?? 0;
  const run = async (apply: boolean) => {
    setBusy(true);
    try {
      const rows = await unwrap(
        commands.relinkLibraryDirectory(
          images.map((item) => item.id),
          original,
          replacement,
          apply,
        ),
      );
      setEntries(rows);
      if (apply) {
        setApplied(true);
        await onApplied();
      }
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      title={`替换目录前缀 · ${images.length} 张失效参考图`}
      width="w700"
      onClose={onClose}
      footer={
        <>
          <button
            type="button"
            className="btn sm"
            disabled={busy || !original.trim() || !replacement || applied}
            onClick={() => void run(false)}
          >
            {busy ? "正在检查…" : "预览匹配"}
          </button>
          <button
            type="button"
            className="btn sm pri"
            disabled={busy || matched === 0 || applied}
            onClick={() => void run(true)}
          >
            应用到 {matched} 张
          </button>
        </>
      }
    >
      <p className="fs12 t2">
        保留原编号、分类、用量和冷却。相对路径和文件内容都匹配时才更新链接。
      </p>
      <label className="fx ac gap8 fs12">
        原目录
        <input
          className="inp f1"
          value={original}
          disabled={busy || applied}
          onChange={(event) => {
            setOriginal(event.target.value);
            setEntries(null);
          }}
        />
      </label>
      <div className="fx ac gap8 mt10">
        <button
          type="button"
          className="btn sm"
          disabled={busy || applied}
          onClick={async () => {
            try {
              const chosen = await unwrap(commands.pickOutputDir());
              if (chosen) {
                setReplacement(chosen);
                setEntries(null);
              }
            } catch (error) {
              toast.error(String(error));
            }
          }}
        >
          选择新目录…
        </button>
        <span className="mono fs11">{replacement}</span>
      </div>
      {entries && (
        <div className="mt10" style={{ maxHeight: "42vh", overflow: "auto" }}>
          <strong className="fs12">
            {applied ? "已更新" : "可匹配"} {matched} 张 · 未匹配 {entries.length - matched} 张
          </strong>
          {entries.map((entry) => (
            <p className="fs11 mt10" key={entry.imageId}>
              #{entry.imageId} {entry.oldPath}
              <br />
              {entry.newPath ?? entry.issue}
            </p>
          ))}
        </div>
      )}
    </Modal>
  );
}
