import { Modal } from "@/components/ui/Modal";
import { PosterDraftDialog } from "@/features/settings/PosterDraftDialog";
import { commands, unwrap } from "@/lib/ipc";
import { useState } from "react";
import { toast } from "sonner";

export function PosterBatchLink({
  batchId,
  workId = null,
}: { batchId: number; workId?: number | null }) {
  const [ids, setIds] = useState<string[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [loading, setLoading] = useState(false);
  const open = async () => {
    setLoading(true);
    try {
      const [found, settings] = await Promise.all([
        unwrap(commands.posterDraftsForImageBatch(batchId, workId)),
        unwrap(commands.getPosterSettings()),
      ]);
      setIds(found);
      setEnabled(Boolean(settings.autoImport));
      if (found.length === 1) setSelected(found[0] ?? null);
      if (found.length === 0)
        toast.info("还没有该范围的 Poster 交付记录。通过验收并加入交付后可在这里查看。");
    } catch (error) {
      toast.error(String(error));
    } finally {
      setLoading(false);
    }
  };

  return (
    <>
      <button type="button" className="btn xs" disabled={loading} onClick={() => void open()}>
        {loading ? "正在查找…" : "Poster 交付记录"}
      </button>
      {ids.length > 1 && !selected && (
        <Modal title="选择 Poster 交付批次" onClose={() => setIds([])}>
          {ids.map((id) => (
            <button key={id} type="button" className="btn sm mt6" onClick={() => setSelected(id)}>
              {id}
            </button>
          ))}
        </Modal>
      )}
      {selected && (
        <PosterDraftDialog
          batchId={selected}
          enabled={enabled}
          onChange={() => {}}
          onClose={() => {
            setSelected(null);
            setIds([]);
          }}
        />
      )}
    </>
  );
}
