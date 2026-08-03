import { type DeliverySettings, commands, unwrap } from "@/lib/ipc";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 设置页「交付」区块。
 *
 * 交付**没有按钮**：验收通过即交付。这里只回答「交到哪里」——那个目录是
 * GenDesk 与 `自动发布` 之间的全部契约。
 *
 * 一次验收 = 一个交付批：
 * `交付-YYMMDD-HHMM/{风格}-{SKU}/{语义名}-{uid}.jpg` + `manifest.jsonl` + 最后写的
 * `READY.txt`。对方读 manifest，**不解析文件名**——从中文文件名反推 SKU 是静默失败。
 */
export function DeliverySection() {
  const [s, setS] = useState<DeliverySettings | null>(null);

  useEffect(() => {
    void unwrap(commands.getDeliverySettings())
      .then(setS)
      .catch(() => {});
  }, []);

  const save = async (root: string) => {
    try {
      setS(await unwrap(commands.updateDeliverySettings({ root })));
    } catch (e) {
      toast.error(String(e));
    }
  };

  if (!s) return null;

  return (
    <section className="sec">
      <div className="sechead">
        <span className="fw6 fs13">交付</span>
        <span className="pcap">验收通过即交付 · manifest + READY</span>
      </div>

      <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        交付根 · 自动发布从这里取图
      </div>
      <div className="fx ac gap10">
        <div className="pathwell f1">{s.root || "—"}</div>
        <button
          type="button"
          className="btn sm"
          onClick={async () => {
            const picked = await unwrap(commands.pickDeliveryRoot()).catch(() => null);
            if (picked) await save(picked);
          }}
        >
          <FolderOpen className="ic12" />
          更改目录
        </button>
        <button
          type="button"
          className="btn sm gho"
          onClick={() => void unwrap(commands.openDeliveryDir()).catch(() => {})}
        >
          打开
        </button>
      </div>
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.8 }}>
        一次验收 = 一个交付批：<span className="chip">交付-YYMMDD-HHMM/</span> 内按{" "}
        <span className="chip">风格-SKU</span> 分子目录，文件名是{" "}
        <span className="chip">语义名-UID.jpg</span>；整批拷完写{" "}
        <span className="chip">manifest.jsonl</span>，<b>最后</b>写{" "}
        <span className="chip">READY.txt</span>。
        <br />
        <b>交付即交出所有权</b>：对方看见 READY 就会把整个目录搬走，此后作品库里那张图会
        显示「源文件缺失」——那是正常的。需要时可在作品库点「重新导出」， 它从资产区快照重出并
        <b>复用原来的 UID</b>。
      </div>
    </section>
  );
}
