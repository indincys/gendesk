import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { Stepper, Toggle } from "@/components/ui/Stepper";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import {
  type ApiKeyView,
  type DataDirInfo,
  commands,
  subscribeBackupProgress,
  unwrap,
} from "@/lib/ipc";
import { useAppVersion } from "@/lib/useAppVersion";
import { cn } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useSettingsStore } from "@/stores/settings";
import { FolderOpen, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

export function SettingsPage() {
  const settings = useSettingsStore((s) => s.settings);
  const loadSettings = useSettingsStore((s) => s.load);
  const updateSettings = useSettingsStore((s) => s.update);

  const version = useAppVersion();
  const [keys, setKeys] = useState<ApiKeyView[]>([]);
  const [showAdd, setShowAdd] = useState(false);
  const [confirmDel, setConfirmDel] = useState<ApiKeyView | null>(null);
  // E19：数据目录信息 + 备份导出进度。
  const [dataDir, setDataDir] = useState<DataDirInfo | null>(null);
  const [backup, setBackup] = useState<{ done: number; total: number } | null>(null);
  // 队列是否有在途任务（run/retry）——运行中禁止备份（避免边写边打包）。
  const queueRunning = useEngineStore((s) => Object.values(s.summaries).some((b) => b.running > 0));

  const loadKeys = useCallback(async () => {
    try {
      setKeys(await unwrap(commands.listApiKeys()));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void loadSettings();
    void loadKeys();
    void (async () => {
      setDataDir(await unwrap(commands.dataDirInfo()).catch(() => null));
    })();
  }, [loadSettings, loadKeys]);

  const openDataDir = async () => {
    await unwrap(commands.openDataDir()).catch((e) => toast.error(String(e)));
  };

  const exportBackup = async () => {
    if (backup) return; // 进行中
    if (queueRunning) {
      toast.error("队列运行中，请先暂停队列再导出备份");
      return;
    }
    setBackup({ done: 0, total: 0 });
    // 订阅进度事件，导出结束即反订阅。
    const unsub = await subscribeBackupProgress((p) => {
      setBackup({ done: p.done, total: p.total });
    });
    try {
      const path = await unwrap(commands.exportBackup());
      if (path) toast.success(`备份已导出到 ${path}`);
      else toast("已取消导出");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      unsub();
      setBackup(null);
    }
  };

  const patchKeyConcurrency = async (k: ApiKeyView, v: number) => {
    setKeys((cur) => cur.map((x) => (x.id === k.id ? { ...x, concurrencyLimit: v } : x)));
    try {
      await unwrap(
        commands.updateApiKey(k.id, {
          concurrencyLimit: v,
          name: null,
          baseUrl: null,
          model: null,
          rpmLimit: null,
        }),
      );
    } catch {
      void loadKeys();
    }
  };

  const toggleKey = async (k: ApiKeyView) => {
    setKeys((cur) => cur.map((x) => (x.id === k.id ? { ...x, enabled: !x.enabled } : x)));
    await unwrap(commands.setApiKeyEnabled(k.id, !k.enabled)).catch(() => void loadKeys());
  };

  const deleteKey = async (k: ApiKeyView) => {
    await unwrap(commands.deleteApiKey(k.id)).catch(() => {});
    void loadKeys();
    toast("已删除 API Key");
  };

  // E18：恢复被熔断的 Key。
  const recoverKey = async (k: ApiKeyView) => {
    await unwrap(commands.recoverApiKey(k.id)).catch((e) => toast.error(String(e)));
    void loadKeys();
    toast(`已恢复「${k.name || "未命名"}」`);
  };

  // E11：测试已保存 Key 的连接。
  const [testingId, setTestingId] = useState<number | null>(null);
  const testSaved = async (k: ApiKeyView) => {
    setTestingId(k.id);
    try {
      await unwrap(commands.testApiKeySaved(k.id));
      toast.success(`「${k.name || "未命名"}」连接正常`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setTestingId(null);
    }
  };

  const enabledCount = keys.filter((k) => k.enabled).length;

  return (
    <PageScaffold title="设置" caption="API Key · 调度与重试 · 输出 · 通用">
      <div className="swrap">
        {/* ---------------- API Key ---------------- */}
        <section className="sec">
          <div className="sechead">
            <span className="fw6 fs13">API Key</span>
            <span className="cnt">
              {enabledCount}/{keys.length} 启用
            </span>
            <div className="f1" />
            <button type="button" className="btn sm" onClick={() => setShowAdd(true)}>
              <Plus className="ic12" />
              添加 Key
            </button>
          </div>
          <div className="klist">
            <div className="kline khd">
              <span>Key</span>
              <span>Base URL</span>
              <span>模型</span>
              <span>并发 1–10</span>
              <span>成功率</span>
              <span>状态</span>
              <span />
              <span />
              <span />
            </div>
            {keys.map((k) => (
              <div className="kline" key={k.id}>
                <span className="fx ac gap7 ohide">
                  <i className={cn("kd", k.enabled ? "kd-ok" : "kd-off")} />
                  <span className="fw5 nowrap ohide" title={k.maskedKey}>
                    {k.name || "未命名"}
                  </span>
                </span>
                <span className="mono fs10 t3 nowrap ohide">{k.baseUrl}</span>
                <span className="mono fs10 t3 nowrap ohide">{k.model}</span>
                <span>
                  <Stepper
                    value={k.concurrencyLimit}
                    min={1}
                    max={10}
                    onChange={(v) => patchKeyConcurrency(k, v)}
                  />
                </span>
                <span className="mono fs11 t2">
                  {k.sampleCount > 0 ? `${Math.round(k.successRate * 100)}%` : "—"}
                </span>
                <span className="fx ac gap6 ohide">
                  {k.circuitBroken ? (
                    <span className="bdg b-red" title="连续鉴权/欠费失败已自动熔断">
                      已熔断
                    </span>
                  ) : (
                    <span className={cn("bdg", k.enabled ? "b-green" : "b-gray")}>
                      {k.enabled ? "启用" : "停用"}
                    </span>
                  )}
                  {k.rpmLimit != null && <span className="fs10 t3 nowrap">{k.rpmLimit}/min</span>}
                </span>
                {k.circuitBroken ? (
                  <button
                    type="button"
                    className="btn sm"
                    onClick={() => recoverKey(k)}
                    title="清除熔断并重新启用"
                  >
                    恢复
                  </button>
                ) : (
                  <button
                    type="button"
                    className="btn sm gho"
                    disabled={testingId === k.id}
                    onClick={() => testSaved(k)}
                    title="测试连接"
                  >
                    {testingId === k.id ? "测试中…" : "测试"}
                  </button>
                )}
                <Toggle on={k.enabled} onClick={() => toggleKey(k)} />
                <button type="button" className="icb" onClick={() => setConfirmDel(k)} title="删除">
                  <Trash2 className="ic12" />
                </button>
              </div>
            ))}
            {keys.length === 0 && (
              <div className="kline">
                <span className="t3 fs12" style={{ gridColumn: "1 / -1" }}>
                  尚未添加 API Key — 点击右上「添加 Key」接入 GPT-Image 2 兼容端点
                </span>
              </div>
            )}
          </div>
        </section>

        {/* ---------------- 调度与重试 ---------------- */}
        <section className="sec">
          <div className="sechead">
            <span className="fw6 fs13">调度与重试</span>
          </div>
          <div className="fx gap10">
            <div
              className={cn("rc", settings?.scheduleStrategy === "round_robin" && "on")}
              onClick={() =>
                updateSettings({
                  scheduleStrategy: "round_robin",
                  retryCount: null,
                  outputDir: null,
                  motion: null,
                  paused: null,
                })
              }
            >
              <div className="fw5 fs13">平均轮询</div>
              <div className="fs11 t3 mt4" style={{ lineHeight: 1.6 }}>
                任务平均分配到全部可用 Key，吞吐稳定，适合各 Key 质量接近的场景
              </div>
            </div>
            <div
              className={cn("rc", settings?.scheduleStrategy === "success_rate" && "on")}
              onClick={() =>
                updateSettings({
                  scheduleStrategy: "success_rate",
                  retryCount: null,
                  outputDir: null,
                  motion: null,
                  paused: null,
                })
              }
            >
              <div className="fw5 fs13">成功率优先</div>
              <div className="fs11 t3 mt4" style={{ lineHeight: 1.6 }}>
                优先调度历史成功率更高的 Key，失败自动切换，适合 Key 质量参差的场景
              </div>
            </div>
          </div>
          <div className="fx ac gap10 mt14 wrap">
            <span className="fs12 t2 nowrap">失败重试次数</span>
            <Stepper
              value={settings?.retryCount ?? 1}
              min={0}
              max={3}
              onChange={(v) =>
                updateSettings({
                  retryCount: v,
                  scheduleStrategy: null,
                  outputDir: null,
                  motion: null,
                  paused: null,
                })
              }
            />
            <span className="fs11 t3">
              超时 / 限流 / 违规默认各自动重试 1 次并切换可用 Key；再次失败则中断并保留错误原因
            </span>
          </div>
        </section>

        {/* ---------------- 输出与归档 ---------------- */}
        <section className="sec">
          <div className="sechead">
            <span className="fw6 fs13">输出与归档</span>
          </div>
          <div className="fx ac gap10">
            <div className="pathwell f1">{settings?.outputDir || "（默认输出目录）"}</div>
            <button
              type="button"
              className="btn sm"
              onClick={async () => {
                const dir = await unwrap(commands.pickOutputDir()).catch(() => null);
                if (dir)
                  updateSettings({
                    outputDir: dir,
                    scheduleStrategy: null,
                    retryCount: null,
                    motion: null,
                    paused: null,
                  });
              }}
            >
              更改目录
            </button>
          </div>
          <div className="fx ac gap8 mt10 wrap">
            <span className="fs12 t2 nowrap">命名规则</span>
            <span className="chip">参考图名</span>
            <span className="t3">_</span>
            <span className="chip">日期</span>
            <span className="t3">_</span>
            <span className="chip">提示词编号</span>
            <span className="t3 fs12">→</span>
            <span className="chip" style={{ color: "var(--acc2)" }}>
              productA_260708_DZ0001.JPG
            </span>
          </div>
        </section>

        {/* ---------------- 通用 ---------------- */}
        <section className="sec">
          <div className="sechead">
            <span className="fw6 fs13">通用</span>
          </div>
          <div className="fx ac gap10">
            <span className="fs12 t2" style={{ width: 72 }}>
              主题
            </span>
            <div className="seg">
              <span className="sgi on">浅色</span>
              <span className="sgi dis">深色 · V2</span>
            </div>
          </div>
          <div className="fx ac gap10 mt10">
            <span className="fs12 t2" style={{ width: 72 }}>
              动效
            </span>
            <div className="seg">
              <span
                className={cn("sgi", (settings?.motion ?? "standard") === "standard" && "on")}
                onClick={() =>
                  updateSettings({
                    motion: "standard",
                    scheduleStrategy: null,
                    retryCount: null,
                    outputDir: null,
                    paused: null,
                  })
                }
              >
                标准
              </span>
              <span
                className={cn("sgi", settings?.motion === "reduced" && "on")}
                onClick={() =>
                  updateSettings({
                    motion: "reduced",
                    scheduleStrategy: null,
                    retryCount: null,
                    outputDir: null,
                    paused: null,
                  })
                }
              >
                减弱
              </span>
            </div>
            <span className="fs11 t3">跟随系统 prefers-reduced-motion 自动降级</span>
          </div>
          <div className="fx ac gap10 mt14">
            <span className="fs12 t2" style={{ width: 72 }}>
              更新
            </span>
            <span className="fs12 nowrap">当前 {version ? `v${version}` : "…"}</span>
            <button
              type="button"
              className="btn sm"
              onClick={async () => {
                toast("正在检查更新…");
                const v = await unwrap(commands.checkUpdateNow()).catch(() => undefined);
                if (v) toast.success(`发现 v${v} · 已在后台下载完成`);
                else if (v === null) toast("已是最新版本");
              }}
            >
              检查更新
            </button>
          </div>
        </section>

        <section className="sec">
          <div className="sechead">
            <span className="fw6 fs13">数据</span>
            <span className="fs11 t3">数据目录位置 · 备份导出</span>
          </div>
          <div className="fx ac gap10">
            <span className="fs12 t2" style={{ width: 72 }}>
              数据目录
            </span>
            <span className="mono fs11 t3 f1 ohide nowrap" title={dataDir?.root ?? ""}>
              {dataDir?.root ?? "…"}
            </span>
            <button type="button" className="btn sm gho" onClick={openDataDir}>
              <FolderOpen className="ic12" />
              打开目录
            </button>
          </div>
          <div className="fx ac gap10 mt14">
            <span className="fs12 t2" style={{ width: 72 }}>
              备份
            </span>
            {backup ? (
              <div className="fx ac gap8 f1">
                <div className="pg f1">
                  <i
                    style={{
                      width: `${backup.total > 0 ? Math.round((backup.done / backup.total) * 100) : 5}%`,
                    }}
                  />
                </div>
                <span className="mono fs10 t3 nowrap">
                  {backup.done}/{backup.total || "…"}
                </span>
              </div>
            ) : (
              <>
                <button
                  type="button"
                  className="btn sm"
                  disabled={queueRunning}
                  onClick={exportBackup}
                >
                  导出备份（zip）
                </button>
                <span className="fs11 t3">
                  {queueRunning
                    ? "队列运行中不可备份，请先暂停队列"
                    : "打包数据库与全部资产到所选 zip；导出前自动检查点保证一致"}
                </span>
              </>
            )}
          </div>
        </section>
      </div>

      {showAdd && (
        <AddKeyModal
          onClose={() => setShowAdd(false)}
          onAdded={() => {
            setShowAdd(false);
            void loadKeys();
          }}
        />
      )}
      {confirmDel && (
        <ConfirmModal
          title="删除 API Key"
          desc={`确定删除「${confirmDel.name || "未命名"}」？该 Key 的凭据会从系统钥匙串移除。`}
          confirmLabel="删除"
          danger
          onConfirm={() => deleteKey(confirmDel)}
          onClose={() => setConfirmDel(null)}
        />
      )}
    </PageScaffold>
  );
}

function AddKeyModal({ onClose, onAdded }: { onClose: () => void; onAdded: () => void }) {
  const [alias, setAlias] = useState("");
  const [key, setKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("gpt-image-2");
  const [concurrency, setConcurrency] = useState("2");
  const [rpm, setRpm] = useState(""); // 空 = 不限速
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);

  const test = async () => {
    if (!key.trim() || !baseUrl.trim()) {
      toast.error("请先填写 Key 与 Base URL");
      return;
    }
    setTesting(true);
    try {
      await unwrap(commands.testApiKey(baseUrl.trim(), key.trim()));
      toast.success("连接正常，可以保存");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (!key.trim() || !baseUrl.trim()) {
      toast.error("Key 与 Base URL 必填");
      return;
    }
    setBusy(true);
    try {
      await unwrap(
        commands.addApiKey({
          alias: alias.trim(),
          key: key.trim(),
          baseUrl: baseUrl.trim(),
          model: model.trim() || "gpt-image-2",
          concurrencyLimit: Number(concurrency) || 2,
          rpmLimit: rpm.trim() ? Number(rpm) : null,
        }),
      );
      toast("已添加 API Key");
      onAdded();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="添加 API Key"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">按 OpenAI 兼容端点接入 · GPT-Image 2</span>
          <div className="f1" />
          <button type="button" className="btn" onClick={test} disabled={testing || busy}>
            {testing ? "测试中…" : "测试连接"}
          </button>
          <button type="button" className="btn pri" onClick={save} disabled={busy}>
            保存
          </button>
        </>
      }
    >
      <div className="col gap10">
        <Field label="别名">
          <input
            className="inp"
            placeholder="例如：主力 · 直连"
            value={alias}
            onChange={(e) => setAlias(e.target.value)}
          />
        </Field>
        <Field label="API Key">
          <input
            className="inp mono"
            placeholder="sk-…"
            value={key}
            onChange={(e) => setKey(e.target.value)}
          />
        </Field>
        <Field label="Base URL">
          <input
            className="inp mono"
            placeholder="https://api.example.com/v1"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
          />
        </Field>
        <div className="fx gap10">
          <div className="col gap4 f1">
            <span className="fs11 t3">模型</span>
            <input className="inp mono" value={model} onChange={(e) => setModel(e.target.value)} />
          </div>
          <div className="col gap4" style={{ width: 90 }}>
            <span className="fs11 t3">并发上限</span>
            <input
              className="inp mono"
              value={concurrency}
              onChange={(e) => setConcurrency(e.target.value)}
            />
          </div>
          <div className="col gap4" style={{ width: 110 }}>
            <span className="fs11 t3">RPM（可选）</span>
            <input
              className="inp mono"
              placeholder="不限"
              value={rpm}
              onChange={(e) => setRpm(e.target.value)}
            />
          </div>
        </div>
      </div>
    </Modal>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="col gap4">
      <span className="fs11 t3">{label}</span>
      {children}
    </div>
  );
}
