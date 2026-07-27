import { V2vVideo } from "@/features/v2v/V2vVideo";
import { type Row, STAGE_META, fmtClock, fmtDur } from "@/features/v2v/model";
import { assetSrc } from "@/lib/img";
import { cn } from "@/lib/utils";
import { Check, Image as ImageIcon, Maximize2, RotateCcw, Undo2, X } from "lucide-react";

/**
 * 右侧详情栏 —— 「这一条的账」与「这一条的历程」。
 *
 * 存在的理由是那句反复出现的怀疑：**这条到底花没花钱、走的哪个模型、等了多久**。
 * 原来只能开弹窗一条一条看，而看片流里每秒就要判一条 —— 弹窗会把节奏整个打断。
 * 故这里是常驻的、跟着光标走的，判定按钮就在它上面。
 */
export function V2vInspector({
  row,
  posText,
  showFirstFrame,
  onToggleFrame,
  onEnterReview,
  onPass,
  onReject,
  onRerun,
  onRewrite,
  onResume,
  onPack,
  busy,
}: {
  row: Row | null;
  /** 「第 3 / 46 条」—— 待验收序列里的位置。 */
  posText: string;
  showFirstFrame: boolean;
  onToggleFrame: () => void;
  onEnterReview: () => void;
  onPass: () => void;
  onReject: () => void;
  onRerun: () => void;
  onRewrite: () => void;
  onResume: () => void;
  onPack: () => void;
  busy: boolean;
}) {
  if (!row) {
    return (
      <div className="vinsp">
        <div className="fs11 t3" style={{ padding: "24px 4px", lineHeight: 1.8 }}>
          选中一条看它的账与历程。
          <br />
          J/K 上下移动 · ⌥\ 收起本栏。
        </div>
      </div>
    );
  }

  const c = row.clip;
  const meta = STAGE_META[row.stage];
  const video = assetSrc(c.videoPath);
  const frame = assetSrc(c.thumbPath);
  const hint = hintFor(row);

  return (
    <div className="vinsp sc">
      <div className="fx ac gap6">
        <span className="pid">{c.promptCode}</span>
        <span className="vstg" style={{ background: meta.bg, color: meta.fg }}>
          {meta.label}
        </span>
        <span className="f1" />
        <span className="fs10 t3 mono">{posText}</span>
      </div>
      <div className="fs10 t3 nowrap ohide mt5">
        {c.batchId == null ? "无批次" : `#${c.batchId}`} · {c.groupName || "未分组"}
      </div>

      {/* 小窗默认 9:16 竖幅 —— 出的片子基本都是竖版，横幅画框会把它压成中间一条，
          左右两大块全是黑边，而这一栏的宽度本来就只有 268px，浪费不起。 */}
      {showFirstFrame ? (
        <div className="vstage pt mt8">
          {frame ? (
            <img src={frame} alt="首帧原图" className="vstageimg" />
          ) : (
            <span className="vstagenote">首帧原图不可用</span>
          )}
        </div>
      ) : video ? (
        // 循环 + 静音自动播放：验收判的是「动起来之后还对不对」，
        // 每条都要手点播放会让一次 46 条的验收多出 46 次点击。
        <V2vVideo className="mt8" src={video} fps={c.fps} portrait videoKey={c.id} />
      ) : (
        <div className="vstage pt mt8">
          <span className="vstagenote">
            {row.stage === "run" ? "尚无成片 · 生成中" : "尚无成片"}
          </span>
        </div>
      )}
      {(c.width != null || c.durationSec != null) && !showFirstFrame && (
        <div className="fs10 t3 mt5">
          {c.width}×{c.height}
          {c.durationSec != null && ` · ${c.durationSec.toFixed(1)}s`}
          {c.fps != null && ` · ${Math.round(c.fps)}fps`}
        </div>
      )}

      <div className="fx gap6 mt6">
        <button
          type="button"
          className={cn("btn xs f1", showFirstFrame && "pri")}
          onClick={onToggleFrame}
          disabled={!frame}
        >
          <ImageIcon className="ic12" />
          对照首帧 <span className="kh">F</span>
        </button>
        <button type="button" className="btn xs f1" onClick={onEnterReview} disabled={!video}>
          <Maximize2 className="ic12" />
          全屏看片 <span className="kh">⏎</span>
        </button>
      </div>

      {row.stage === "rev" && (
        <div className="fx gap5 mt8">
          <button type="button" className="btn sm okb f1" disabled={busy} onClick={onPass}>
            <Check className="ic12" />
            通过 <span className="kh">空格</span>
          </button>
          <button type="button" className="btn sm dngo f1" disabled={busy} onClick={onReject}>
            <X className="ic12" />
            不通过 <span className="kh">X</span>
          </button>
        </div>
      )}
      {row.stage === "pass" && (
        <div className="fx gap5 mt8">
          <button
            type="button"
            className="btn sm pri f1"
            disabled={busy || c.inAssetLib}
            onClick={onPack}
            title="打包成视频型素材包（1 视频 + 封面）接上发布链"
          >
            {c.inAssetLib ? "已入资产库" : "入资产库"} <span className="kh">A</span>
          </button>
        </div>
      )}
      {row.signals.has("timeout") && (
        <div className="fx gap5 mt8">
          <button type="button" className="btn sm pri f1" disabled={busy} onClick={onResume}>
            继续等待 <span className="kh">W</span>
          </button>
        </div>
      )}

      <div className="fx gap5 mt5">
        <button type="button" className="btn xs f1" disabled={busy} onClick={onRerun}>
          <RotateCcw className="ic12" />
          重跑 <span className="kh">R</span>
        </button>
        <button type="button" className="btn xs f1" disabled={busy} onClick={onRewrite}>
          <Undo2 className="ic12" />
          退回改写 <span className="kh">E</span>
        </button>
      </div>

      {hint && (
        <div className={cn("vhint", hint.tone)} style={{ marginTop: 7 }}>
          {hint.text}
        </div>
      )}

      <div className="vsec">这一条的账</div>
      <div className="fx col gap5 mt5">
        <Fact k="模型型号（我们发的）" v={row.modelFull ?? "跟随 CLI 默认"} />
        <Fact k="计费型号（即梦回执）" v={c.benefitType ?? "—"} />
        <Fact k="submit_id" v={c.submitId ?? "—"} />
      </div>
      <div className="vfacts mt5">
        <Fact
          k="实际扣费"
          v={
            c.creditCount != null
              ? `${c.creditCount} 额度`
              : c.submitCredit != null
                ? `${c.submitCredit} 额度（提交回执）`
                : row.signals.has("phantom")
                  ? "未计费"
                  : "—"
          }
        />
        <Fact
          k="预估单价"
          v={row.estimate == null ? "未实测" : `${row.estimate} / 条`}
          tone={row.estimate == null ? "wr" : undefined}
        />
        <Fact
          k="规格"
          v={
            row.resolution && row.duration
              ? `${row.resolution} · ${row.duration}s`
              : (row.resolution ?? "CLI 默认")
          }
        />
        <Fact k="已等" v={row.waitSecs === 0 ? "—" : fmtDur(row.waitSecs)} />
        <Fact
          k="上次查询"
          v={row.polledAgo == null ? "—" : `${fmtDur(row.polledAgo)}前`}
          tone={row.polledAgo != null && row.polledAgo > 1800 ? "wr" : undefined}
        />
        <Fact k="尝试" v={`第 ${Math.max(1, c.attempt)} 次`} />
      </div>

      <div className="vsec">这一条的历程</div>
      <div className="mt4">
        {trailOf(row).map((t) => (
          <div key={t.what} className="vtrail">
            <span className="dot" style={{ background: t.color }} />
            <span className="at">{t.at}</span>
            <span className="what">{t.what}</span>
          </div>
        ))}
      </div>

      <div className="vsec">视频提示词</div>
      <div className="vprompt mt4">{c.videoPrompt ?? "（还没有，等 skill 写回）"}</div>

      {c.errorMessage && (
        <>
          <div className="vsec">失败原文</div>
          <div className="vprompt mt4" style={{ color: "var(--er)" }}>
            {c.errorMessage}
          </div>
        </>
      )}
      <div style={{ height: 8 }} />
    </div>
  );
}

function Fact({ k, v, tone }: { k: string; v: string; tone?: "wr" | undefined }) {
  return (
    <div className="vfact">
      <div className="k">{k}</div>
      <div className="v" style={tone === "wr" ? { color: "var(--wr2)" } : undefined} title={v}>
        {v}
      </div>
    </div>
  );
}

/**
 * 上下文提示 —— 只在**处置方向会搞反**的时候出现。
 *
 * 超时与幽灵单是一对相反的事：一个额度已扣（该等），一个从未计费（该重跑）。
 * 指错方向的代价是真金白银，所以这两句必须写在按钮旁边，而不是躺在文档里。
 */
function hintFor(row: Row): { text: string; tone: "wr" | "er" } | null {
  if (row.signals.has("timeout")) {
    return {
      tone: "wr",
      text: "超时只是我们这边不等了：额度已扣、即梦那边还在跑。点「继续等待」沿用原提交单；重跑等于再花一份钱买同一条视频。",
    };
  }
  if (row.signals.has("phantom")) {
    return {
      tone: "er",
      text: "两个信号同时缺席（无队列位次、无扣费回执）—— 即梦接了单但从未入队、从未计费。重跑不花钱。",
    };
  }
  if (row.slow) {
    return {
      tone: "wr",
      text: "这一条已超本批中位等待时长的 3 倍。退避轮询已放缓到十分钟一次，不必手动催。",
    };
  }
  if (row.vip) {
    return {
      tone: "wr",
      text: `走的是 vip 通道${row.estimate == null ? "" : `：约 ${row.estimate} 额度/条`}，非 vip 同规格实测只要 8。vip 买到的只是不排队。`,
    };
  }
  return null;
}

/** 这一条走过的四个时刻。没发生的一律留白，不编时间。 */
function trailOf(row: Row): { at: string; what: string; color: string }[] {
  const c = row.clip;
  const out: { at: string; what: string; color: string }[] = [
    { at: fmtClock(c.createdAt), what: "图片验收通过 · 自动入队待改写", color: "var(--sg-pass)" },
  ];
  out.push(
    c.rewroteAt != null
      ? { at: fmtClock(c.rewroteAt), what: "改写结果写回 · 进待提交", color: "var(--acc)" }
      : { at: "—", what: "等 skill 写回改写结果", color: "var(--line2)" },
  );
  out.push(
    c.firstSubmittedAt != null
      ? {
          at: fmtClock(c.firstSubmittedAt),
          what: `提交到即梦${c.submitCredit != null ? ` · 回执计费 ${c.submitCredit}` : " · 回执未带计费"}`,
          color: "var(--acc)",
        }
      : { at: "—", what: "等你放行提交", color: "var(--line2)" },
  );
  if (row.stage === "fail") {
    out.push({
      at: c.finishedAt == null ? "—" : fmtClock(c.finishedAt),
      what: c.errorType === "timeout" ? "判超时 · 提交单仍有效" : `判死 · ${c.errorType ?? "失败"}`,
      color: "var(--er)",
    });
  } else {
    out.push(
      c.finishedAt != null
        ? {
            at: fmtClock(c.finishedAt),
            what: `出片落盘${c.width != null ? ` ${c.width}×${c.height}` : ""} · 进待验收`,
            color: "var(--sg-rev)",
          }
        : { at: "—", what: "等即梦出片", color: "var(--line2)" },
    );
  }
  out.push(
    c.reviewedAt != null
      ? {
          at: fmtClock(c.reviewedAt),
          what: row.stage === "pass" ? "验收通过" : "验收不通过 · 成片进废纸篓",
          color: row.stage === "pass" ? "var(--sg-pass)" : "var(--t3)",
        }
      : { at: "—", what: "等你判定", color: "var(--line2)" },
  );
  return out;
}
