import { DescriptionHint } from "@/components/ui/Tooltip";
import { type ModelInfo, type SessionInfo, type V2vSettings, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { AlertTriangle, Check, FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 设置页「图生视频」区块。
 *
 * 三件事在这里定：**交接目录在哪**（skill 要把它写死）、**成片交付到哪**、
 * 与**默认生成参数**。参数留空即不发高级 flag，走即梦 CLI 自己的默认路径。
 *
 * 注意「默认」二字：这里的参数只作用于新条目。已经在流水线里的条目要改参数，
 * 在工作台改（选中后底栏的参数条 / 右侧详情栏改单条）—— 三处作用域各自在自己的
 * 标签上说清楚，那是 v0.22.0 修的一处歧义。
 */
export function V2vSection() {
  const [s, setS] = useState<V2vSettings | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [sessions, setSessions] = useState<SessionInfo[] | null>(null);
  /** 当前设置实际会执行哪个文件（后端探测结果）；null = 没找到。 */
  const [resolved, setResolved] = useState<string | null>(null);
  /** 当前生效的成片交付目录（留空时是回落后的默认，故要问后端而不是显示空串）。 */
  const [clipsDir, setClipsDir] = useState<string>("");

  const refreshClipsDir = () =>
    void unwrap(commands.v2vClipsDir())
      .then(setClipsDir)
      .catch(() => setClipsDir(""));

  const refreshResolved = () =>
    void unwrap(commands.resolveV2vBin())
      .then(setResolved)
      .catch(() => setResolved(null));

  // biome-ignore lint/correctness/useExhaustiveDependencies: 两个 refresh 是稳定的模块级闭包，只在挂载时跑一次
  useEffect(() => {
    void unwrap(commands.getV2vSettings())
      .then(setS)
      .catch(() => {});
    void unwrap(commands.v2vModels())
      .then(setModels)
      .catch(() => setModels([]));
    refreshResolved();
    refreshClipsDir();
  }, []);

  const save = async (p: Partial<V2vSettings>) => {
    if (!s) return;
    try {
      setS(await unwrap(commands.updateV2vSettings({ ...s, ...p })));
    } catch (e) {
      // 后端拒绝（非法参数组合）→ 提示并回读，避免界面停在一个没存进去的值上。
      toast.error(String(e));
      setS(await unwrap(commands.getV2vSettings()));
    }
    if (p.bin !== undefined) refreshResolved();
    if (p.clipsOutputDir !== undefined) refreshClipsDir();
  };

  useEffect(() => {
    // 会话列表不阻塞渲染；读不到就退化成手填数字（下面那个 input 仍在）。
    void unwrap(commands.v2vSessions())
      .then(setSessions)
      .catch(() => setSessions([]));
  }, []);

  if (!s) return null;
  const picked = models.find((m) => m.modelVersion === s.modelVersion);

  return (
    <section className="sec">
      <div className="sechead">
        <span className="fw6 fs13">视频生成</span>
        <span className="pcap">目录 · CLI · 默认参数</span>
      </div>

      <div className="fx ac gap4" style={{ marginBottom: 6 }}>
        <span className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
          交接目录
        </span>
        <DescriptionHint label="交接目录说明">
          待改写工单写入 v2v/待改写；skill 将结果写回 v2v/已改写，GenDesk 会自动收录。
        </DescriptionHint>
      </div>
      <div className="fx ac gap10">
        <div className="pathwell f1">{s.handoffRoot}</div>
        <button
          type="button"
          className="btn sm"
          onClick={async () => {
            const dir = await unwrap(commands.pickHandoffRoot()).catch(() => null);
            if (dir) await save({ handoffRoot: dir });
          }}
        >
          <FolderOpen className="ic12" />
          更改目录
        </button>
      </div>
      <div className="fx ac gap4 mt14" style={{ marginBottom: 6 }}>
        <span className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
          成片目录
        </span>
        <DescriptionHint label="成片目录说明">
          验收通过后按组名与编号复制到这里；应用内成片仍会保留。
        </DescriptionHint>
      </div>
      <div className="fx ac gap10">
        <div className="pathwell f1">{clipsDir || "—"}</div>
        <button
          type="button"
          className="btn sm"
          onClick={async () => {
            const dir = await unwrap(commands.pickClipsOutputDir()).catch(() => null);
            if (dir) await save({ clipsOutputDir: dir });
          }}
        >
          <FolderOpen className="ic12" />
          更改目录
        </button>
        <button
          type="button"
          className="btn sm gho"
          onClick={() =>
            void unwrap(commands.openClipsOutputDir()).catch((e) => toast.error(String(e)))
          }
        >
          打开
        </button>
      </div>
      <div className="fx ac gap4 mt14" style={{ marginBottom: 6 }}>
        <span className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
          即梦 CLI
        </span>
        <DescriptionHint label="CLI 路径说明">
          留空时自动探测；从访达启动可能读不到终端 PATH，探测失败时请选择绝对路径。
        </DescriptionHint>
      </div>
      <div className="fx ac gap10">
        <input
          className="inp f1"
          value={s.bin}
          placeholder="留空自动探测"
          onChange={(e) => setS({ ...s, bin: e.target.value })}
          onBlur={() => void save({ bin: s.bin ?? "" })}
        />
        <button
          type="button"
          className="btn sm"
          onClick={async () => {
            const f = await unwrap(commands.pickDreaminaBin()).catch(() => null);
            if (f) await save({ bin: f });
          }}
        >
          <FolderOpen className="ic12" />
          选择文件
        </button>
      </div>
      {/* 「路径填什么」不该由人回答：直接把解析结果摆出来，说清实际会执行哪个文件。 */}
      <div className="fs11 t3 mt6">
        {resolved ? (
          <span className="fx ac gap6">
            <Check className="ic12" style={{ color: "var(--ok)" }} />
            实际会执行：<span className="chip">{resolved}</span>
          </span>
        ) : (
          <span className="fx ac gap6" style={{ color: "var(--wr)" }}>
            <AlertTriangle className="ic12" />
            未找到即梦 CLI
          </span>
        )}
      </div>

      <div className="fx ac gap10 mt14">
        <span className="fs12 fw5">后台轮询</span>
        <div className="seg">
          <span
            className={cn("sgi", s.pollEnabled && "on")}
            onClick={() => void save({ pollEnabled: true })}
          >
            开
          </span>
          <span
            className={cn("sgi", !s.pollEnabled && "on")}
            onClick={() => void save({ pollEnabled: false })}
          >
            关
          </span>
        </div>
        <DescriptionHint label="后台轮询说明">
          关闭后停止自动取回；已提交任务不会丢失，重新开启后继续查询。
        </DescriptionHint>
      </div>

      {/* 超时上限。默认不限 —— 判死一条还在跑的任务代价是钱（额度已扣、即梦那边照跑），
          而多等的代价只是看板上多几条「已提交」。两边不对等。 */}
      <div className="fx ac gap10 mt10 wrap">
        <span className="fs12 fw5">判超时</span>
        <select
          className="inp"
          style={{ width: 150 }}
          value={s.timeoutHours ?? ""}
          onChange={(e) =>
            void save({ timeoutHours: e.target.value === "" ? null : Number(e.target.value) })
          }
        >
          <option value="">不限（推荐）</option>
          <option value={3}>3 小时</option>
          <option value={12}>12 小时</option>
          <option value={24}>24 小时</option>
        </select>
        <DescriptionHint label="超时规则说明">
          即梦排队可能持续数小时；不限会持续等待，轮询频率会自动降低。
        </DescriptionHint>
        {s.timeoutHours != null && <span className="bdg b-amber">超时后恢复可能再次计费</span>}
      </div>

      <div className="fx ac gap4 mt14" style={{ marginBottom: 6 }}>
        <span className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
          默认生成参数
        </span>
        <DescriptionHint label="默认参数说明">
          只影响新任务；改写结果给出的单条参数优先。
        </DescriptionHint>
      </div>
      <div className="fx ac gap10 wrap">
        <select
          className="inp"
          value={s.modelVersion}
          onChange={(e) =>
            // 换模型即清掉时长与分辨率：留着上一个模型的值必然撞它的约束。
            void save({ modelVersion: e.target.value, duration: null, videoResolution: "" })
          }
        >
          <option value="">默认（不发高级参数，跟随 CLI）</option>
          {models.map((m) => (
            <option key={m.modelVersion} value={m.modelVersion}>
              {m.modelVersion}
              {m.creditAtMin === null
                ? ""
                : `（${m.creditAtMin} 额度起/条 · ${m.minDuration}s ${m.resolutions[0]}）`}
            </option>
          ))}
        </select>
        {picked && (
          <>
            <input
              className="inp"
              style={{ width: 110 }}
              type="number"
              min={picked.minDuration}
              max={picked.maxDuration}
              placeholder={`${picked.minDuration}–${picked.maxDuration} 秒`}
              value={s.duration ?? ""}
              onChange={(e) =>
                setS({ ...s, duration: e.target.value === "" ? null : Number(e.target.value) })
              }
              onBlur={() => void save({ duration: s.duration ?? null })}
            />
            <select
              className="inp"
              value={s.videoResolution}
              onChange={(e) => void save({ videoResolution: e.target.value })}
            >
              <option value="">{picked.resolutions[0]}（默认）</option>
              {picked.resolutions.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </>
        )}
        {/* 通道 = 即梦会话。原先这里只是个裸数字输入框 ——「这个数字是哪条会话」
            在应用里根本无从得知，于是它等于不可用。读不到列表时才退回手填。 */}
        {sessions && sessions.length > 0 ? (
          <select
            className="inp"
            style={{ minWidth: 200 }}
            value={s.session ?? ""}
            onChange={(e) =>
              void save({ session: e.target.value === "" ? null : Number(e.target.value) })
            }
          >
            <option value="">默认会话（0）</option>
            {sessions.map((x) => (
              <option key={x.id} value={x.id}>
                {x.id} · {x.name}
              </option>
            ))}
          </select>
        ) : (
          <input
            className="inp"
            style={{ width: 130 }}
            type="number"
            min={0}
            placeholder="会话 id（可空）"
            value={s.session ?? ""}
            onChange={(e) =>
              setS({ ...s, session: e.target.value === "" ? null : Number(e.target.value) })
            }
            onBlur={() => void save({ session: s.session ?? null })}
          />
        )}
      </div>
    </section>
  );
}
