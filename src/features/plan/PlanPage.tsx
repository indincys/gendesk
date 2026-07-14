import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { Stepper, Toggle } from "@/components/ui/Stepper";
import { assetSrc } from "@/lib/img";
import {
  type AccountView,
  type PreflightReport,
  type PublishSettings,
  type PublishSettingsPatch,
  type SheetDetail,
  type SheetSummary,
  type TaskRowView,
  commands,
  subscribeExportProgress,
  unwrap,
} from "@/lib/ipc";
import { failKindLabel, isShortage, pubTaskVisual, sheetVisual, shortageLabel } from "@/lib/status";
import { cn } from "@/lib/utils";
import { usePublishStore } from "@/stores/publish";
import { ChevronLeft, ChevronRight, FolderOpen, Plus, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

type Tab = "board" | "sheets" | "strategy";

/** 发布计划页（原型 publish.dc.html 发布计划三页签）。看板于 P3 充实。 */
export function PlanPage() {
  const [tab, setTab] = useState<Tab>("sheets");
  const [openSheet, setOpenSheet] = useState<number | null>(null);
  const badges = usePublishStore((s) => s.badges);
  const [paused, setPaused] = useState(false);

  useEffect(() => {
    void unwrap(commands.getPublishSettings()).then((s) => setPaused(s.schedulePaused ?? false));
  }, []);
  const togglePause = async () => {
    const next = !paused;
    try {
      await unwrap(
        commands.updatePublishSettings({
          rootLocal: null,
          rootExec: null,
          pathStyle: null,
          dedupDays: null,
          receiptTimeoutHours: null,
          autogenTime: null,
          warnMaterial: null,
          warnTitle: null,
          warnBody: null,
          accountDailyLimitDefault: null,
          minGapMinutes: null,
          platformMatrix: null,
          tierRules: null,
          timeSlots: null,
          archiveRetentionDays: null,
          schedulePaused: next,
        }),
      );
      setPaused(next);
      toast.success(next ? "排期已暂停（对账与超时扫描照常）" : "排期已恢复");
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <div className="col f1 ohide">
      <div className="phd">
        <span className="ptt">发布计划</span>
        <div className="seg" style={{ marginLeft: 6 }}>
          <span className={cn("sgi", tab === "board" && "on")} onClick={() => setTab("board")}>
            看板
          </span>
          <span
            className={cn("sgi", tab === "sheets" && "on")}
            onClick={() => {
              setTab("sheets");
              setOpenSheet(null);
            }}
          >
            任务单
            {badges.pendingSheets > 0 && (
              <span className="bdg b-amber" style={{ height: 16, padding: "0 5px" }}>
                {badges.pendingSheets}
              </span>
            )}
          </span>
          <span
            className={cn("sgi", tab === "strategy" && "on")}
            onClick={() => setTab("strategy")}
          >
            策略与账号
          </span>
        </div>
        <div className="f1" />
        {/* 节假日暂停：只停自动生成草稿，超时扫描与对账照常——
            回收闭环停了的话，暂停期间已导出的单永远收不回来。 */}
        <span className="fs11 t3">排期</span>
        <Toggle on={!paused} onClick={() => void togglePause()} />
        <span className={cn("fs11", paused ? "b-amber" : "t3")}>
          {paused ? "已暂停" : "运行中"}
        </span>
      </div>

      {paused && (
        <div className="ban" style={{ margin: "10px 18px 0", borderColor: "var(--wr)" }}>
          <span className="f1">
            排期已暂停 — 不再自动生成明日草稿；回执对账与超时扫描照常，手动生成也仍可用
          </span>
        </div>
      )}

      {tab === "board" && (
        <Board
          onOpenSheet={(id) => {
            setTab("sheets");
            setOpenSheet(id);
          }}
        />
      )}
      {tab === "sheets" &&
        (openSheet == null ? (
          <SheetList onOpen={setOpenSheet} />
        ) : (
          <Workbench sheetId={openSheet} onBack={() => setOpenSheet(null)} />
        ))}
      {tab === "strategy" && <StrategyTab />}
    </div>
  );
}

function todayStr(): string {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function Board({ onOpenSheet }: { onOpenSheet: (id: number) => void }) {
  const [dash, setDash] = useState<import("@/lib/ipc").DashboardView | null>(null);
  const [report, setReport] = useState<import("@/lib/ipc").ReportView | null>(null);
  const [showReport, setShowReport] = useState(false);
  const date = todayStr();

  const sheetRev = usePublishStore((s) => s.sheetRev);

  const load = useCallback(async () => {
    try {
      setDash(await unwrap(commands.getDashboard(date)));
    } catch {
      setDash(null);
    }
  }, [date]);
  // 任务单有任何变化（watcher 对账 / 导出 / 人工定态）→ 看板自动跟着走。
  useEffect(() => {
    void load();
  }, [load, sheetRev]);

  if (!dash) {
    return (
      <div className="bigempty" style={{ padding: "72px 20px" }}>
        <div className="fs13 fw5 t2">今日暂无任务单</div>
        <div className="fs12 t3">配置根目录并生成任务单后，看板展示今日发布进度</div>
      </div>
    );
  }

  const sid = dash.sheetId;
  const openReport = async () => {
    if (sid == null) return;
    const r = await unwrap(commands.getReport(sid));
    setReport(r);
    setShowReport(true);
  };

  return (
    <div className="pbody">
      <div className="statrow">
        <div className="statcard">
          <div className="stnum">{dash.plan}</div>
          <div className="stlbl">今日计划任务</div>
        </div>
        <div className="statcard">
          <div className="stnum" style={{ color: "var(--ok)" }}>
            {dash.published}
          </div>
          <div className="stlbl">
            <span className="dt" style={{ background: "var(--ok)" }} />
            已发布
          </div>
        </div>
        <div className="statcard">
          <div className="stnum" style={{ color: "var(--er)" }}>
            {dash.failed}
          </div>
          <div className="stlbl">
            <span className="dt" style={{ background: "var(--er)" }} />
            失败
          </div>
        </div>
        <div
          className={cn("statcard", dash.suspect > 0 && "hl")}
          onClick={() => dash.sheetId != null && dash.suspect > 0 && onOpenSheet(dash.sheetId)}
        >
          <div className="stnum" style={{ color: "var(--wr)" }}>
            {dash.suspect}
          </div>
          <div className="stlbl">
            <span className="dt" style={{ background: "var(--wr)" }} />
            待核对 · 疑似已发
          </div>
        </div>
      </div>

      {dash.suspect > 0 && (
        <div className="ban" style={{ margin: "10px 18px 0" }}>
          <span className="f1">
            {dash.suspect} 个任务疑似已发 — 疑似已发绝不自动重发，需人工到平台后台核实后定态
          </span>
          {sid != null && (
            <button type="button" className="btn sm" onClick={() => onOpenSheet(sid)}>
              去核对
            </button>
          )}
        </div>
      )}

      <div className="fx gap12" style={{ padding: "14px 18px 0", alignItems: "flex-start" }}>
        <div className="card f1" style={{ minWidth: 0 }}>
          <div className="chead">
            <span className="fw6 fs13">按平台完成率</span>
          </div>
          {dash.platforms.map((p) => (
            <div
              key={p.platform}
              className="fx ac gap10"
              style={{ padding: "7px 14px", borderTop: "1px solid var(--line)" }}
            >
              <span className="fs12 fw5 nowrap" style={{ width: 52 }}>
                {p.platformZh}
              </span>
              <div className="pbarw">
                <i style={{ width: `${p.pct}%` }} />
              </div>
              <span className="mono fs11 t2 nowrap" style={{ width: 64, textAlign: "right" }}>
                {p.done}/{p.total}
              </span>
            </div>
          ))}
          {dash.platforms.length === 0 && <div className="txrow t3 fs12">今日无任务</div>}
        </div>
        <div className="card f1" style={{ minWidth: 0 }}>
          <div className="chead">
            <span className="fw6 fs13">账号健康</span>
            <span className="cnt">{dash.accounts.length}</span>
          </div>
          {dash.accounts.map((a) => (
            <div
              key={a.id}
              className="fx ac gap8"
              style={{ padding: "6px 14px", borderTop: "1px solid var(--line)", minHeight: 34 }}
            >
              <span className="bdg b-gray">{a.platformZh}</span>
              <span className="fs12 fw5 f1 nowrap ohide">{a.name}</span>
              <span className="mono fs11 t3 nowrap">
                {a.used}/{a.dailyLimit}
              </span>
              <span
                className={cn(
                  "bdg",
                  a.health === "normal" ? "b-green" : a.health === "circuit" ? "b-red" : "b-gray",
                )}
              >
                {a.health === "normal" ? "正常" : a.health === "circuit" ? "当日熔断" : "停用"}
              </span>
            </div>
          ))}
        </div>
      </div>

      {dash.hasReport && (
        <div className="card" style={{ margin: "12px 18px 24px" }}>
          <div className="chead">
            <span className="fw6 fs13">日报</span>
            <span className="pcap">任务单全部终态后自动关闭并生成</span>
            <div className="f1" />
            <button type="button" className="btn sm" onClick={() => void openReport()}>
              查看日报
            </button>
          </div>
        </div>
      )}

      {showReport && report && <ReportModal report={report} onClose={() => setShowReport(false)} />}
    </div>
  );
}

function ReportModal({
  report,
  onClose,
}: {
  report: import("@/lib/ipc").ReportView;
  onClose: () => void;
}) {
  return (
    <Modal
      title={`日报 · ${report.date}`}
      width="w640"
      onClose={onClose}
      headerExtra={
        <span className="bdg b-green">
          <span className="dt" />
          已关闭
        </span>
      }
    >
      <div className="fx gap10">
        {[
          ["计划", report.plan, undefined],
          ["成功", report.published, "var(--ok)"],
          ["失败", report.failed, "var(--er)"],
          ["成功率", `${report.successRate}%`, undefined],
        ].map(([label, val, color]) => (
          <div key={String(label)} className="rc">
            <div className="stnum" style={{ fontSize: 20, color: color as string }}>
              {val}
            </div>
            <div className="stlbl">{label}</div>
          </div>
        ))}
      </div>
      {report.fails.length > 0 && (
        <>
          <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
            失败清单
          </div>
          <div className="mt6 col gap6">
            {report.fails.map((f) => (
              <div key={f.taskCode} className="fx ac gap8">
                <span className="pid">{f.taskCode}</span>
                <span className="bdg b-red">{failKindLabel(f.kind)}</span>
                <span className="fs12 f1 nowrap ohide">{f.skuCode}</span>
              </div>
            ))}
          </div>
        </>
      )}
      {report.shortage.length > 0 && (
        <>
          <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
            缺料清单
          </div>
          <div className="fs12 mt6">{report.shortage.join("、")}</div>
        </>
      )}
      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
        明日建议
      </div>
      <div className="fs12 mt6" style={{ lineHeight: 1.8 }}>
        {report.tips}
      </div>
    </Modal>
  );
}

// ─────────────────────────────────────────────── 任务单列表

function SheetList({ onOpen }: { onOpen: (id: number) => void }) {
  const [sheets, setSheets] = useState<SheetSummary[]>([]);
  const [genTime, setGenTime] = useState("22:00");
  const refreshBadges = usePublishStore((s) => s.refreshBadges);
  const sheetRev = usePublishStore((s) => s.sheetRev);

  const load = useCallback(async () => {
    setSheets(await unwrap(commands.listSheets()));
    try {
      const s = await unwrap(commands.getPublishSettings());
      setGenTime(s.autogenTime ?? "22:00");
    } catch {
      // ignore
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load, sheetRev]);

  const genTomorrow = async () => {
    const d = new Date();
    d.setDate(d.getDate() + 1);
    const date = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
      d.getDate(),
    ).padStart(2, "0")}`;
    try {
      await unwrap(commands.generateSheet(date));
      toast.success(`已生成 ${date} 任务单草稿`);
      await load();
      void refreshBadges();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <>
      <div className="fx ac gap8" style={{ padding: "10px 18px 8px" }}>
        <span className="fs11 t3">每晚 {genTime} 自动生成明日任务单草稿 · 也可手动触发</span>
        <div className="f1" />
        <button type="button" className="btn sm" onClick={() => void genTomorrow()}>
          <Plus className="ic12" />
          生成明日任务单
        </button>
      </div>
      <div className="f1" style={{ overflow: "auto", minHeight: 0 }}>
        {sheets.map((s) => {
          const v = sheetVisual(s.status);
          const chips: { cls: string; t: string }[] = [];
          if (s.published > 0) chips.push({ cls: "b-green", t: `已发 ${s.published}` });
          if (s.failed > 0) chips.push({ cls: "b-red", t: `失败 ${s.failed}` });
          if (s.suspect > 0) chips.push({ cls: "b-amber", t: `待核对 ${s.suspect}` });
          if (s.shortageCount > 0) chips.push({ cls: "b-gray", t: `缺料 ${s.shortageCount}` });
          return (
            <div key={s.id} className="shrow" onClick={() => onOpen(s.id)}>
              <span className="fw6 fs13 nowrap" style={{ width: 92 }}>
                {s.date}
              </span>
              <span className="fs11 t3 nowrap" style={{ width: 44 }}>
                {s.taskCount} 行
              </span>
              <span className={cn("bdg", v.badgeClass)}>
                <span className="dt" />
                {v.label}
              </span>
              <span className="f1" />
              {chips.map((c) => (
                <span key={c.t} className={cn("bdg", c.cls)}>
                  {c.t}
                </span>
              ))}
              <ChevronRight className="ic12 t3" />
            </div>
          );
        })}
        {sheets.length === 0 && (
          <div className="bigempty" style={{ padding: "56px 20px" }}>
            <div className="fs13 fw5 t2">暂无任务单</div>
            <div className="fs12 t3">点「生成明日任务单」创建草稿；或等每晚定时生成</div>
          </div>
        )}
      </div>
    </>
  );
}

// ─────────────────────────────────────────────── 工作台

function Workbench({ sheetId, onBack }: { sheetId: number; onBack: () => void }) {
  const [detail, setDetail] = useState<SheetDetail | null>(null);
  const [timeEdit, setTimeEdit] = useState<TaskRowView | null>(null);
  const [verify, setVerify] = useState<TaskRowView | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [confirmExport, setConfirmExport] = useState(false);
  const [cancelRow, setCancelRow] = useState<TaskRowView | null>(null);
  const [confirmRegen, setConfirmRegen] = useState(false);
  const refreshBadges = usePublishStore((s) => s.refreshBadges);

  const lastSheetChanged = usePublishStore((s) => s.lastSheetChanged);

  const load = useCallback(async () => {
    setDetail(await unwrap(commands.getSheet(sheetId)));
  }, [sheetId]);
  useEffect(() => {
    void load();
  }, [load]);

  // watcher 对账（回执落盘 → 2s 防抖 → SheetChangedEvent）时自动刷新，只认自己这一单。
  useEffect(() => {
    if (lastSheetChanged?.sheetId === sheetId) void load();
  }, [lastSheetChanged, sheetId, load]);

  if (!detail) return <div className="col f1" />;
  const v = sheetVisual(detail.status);
  const isDraft = detail.status === "draft";
  const isConfirmed = detail.status === "confirmed";
  const isExported = detail.status === "exported" || detail.status === "reconciling";
  // shortage_json 兼装了真·缺料与「补排」提示，分开渲染（含义完全不同）。
  const shortages = detail.shortage.filter((s) => isShortage(s.reason));
  const backfills = detail.shortage.filter((s) => !isShortage(s.reason));

  const regenerate = async () => {
    setConfirmRegen(false);
    await act(() => unwrap(commands.generateSheet(detail.date)), "已按当前素材重新生成");
  };

  const act = async (fn: () => Promise<unknown>, ok?: string) => {
    try {
      await fn();
      if (ok) toast.success(ok);
      await load();
      void refreshBadges();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  // 按 SKU 分组。
  const groups = new Map<number, TaskRowView[]>();
  for (const r of detail.rows) {
    const arr = groups.get(r.skuId) ?? [];
    arr.push(r);
    groups.set(r.skuId, arr);
  }

  return (
    <>
      <div className="fx ac gap8" style={{ padding: "10px 18px 0" }}>
        <button type="button" className="icb" onClick={onBack} aria-label="返回">
          <ChevronLeft className="ic12" />
        </button>
        <span className="fw6 fs13 nowrap">任务单 · {detail.date}</span>
        <span className="fs11 t3">
          {detail.rows.length} 行 · {groups.size} 个 SKU
        </span>
        <span className={cn("bdg", v.badgeClass)}>
          <span className="dt" />
          {v.label}
        </span>
        <div className="f1" />
        {isDraft && (
          <button
            type="button"
            className="btn sm gho"
            title="按当前素材/频率/账号重算这一天（会清掉人工调整）"
            onClick={() => (detail.edited ? setConfirmRegen(true) : void regenerate())}
          >
            <RefreshCw className="ic12" />
            重新生成
          </button>
        )}
        {isDraft && (
          <button type="button" className="btn sm gho" onClick={() => setAddOpen(true)}>
            <Plus className="ic12" />
            增补任务行
          </button>
        )}
        {isExported && (
          <button
            type="button"
            className="btn sm"
            onClick={() =>
              void act(async () => {
                const r = await unwrap(commands.importReceipts(sheetId));
                toast.success(`对账完成：已发布 ${r.published} · 失败 ${r.failed}`);
                if (r.retiredPacks > 0)
                  toast.warning(`${r.retiredPacks} 个素材包因「素材不合规」已退役`);
                for (const name of r.loginFailAccounts)
                  toast.error(`账号「${name}」登录失效，需人工处理（不会自动重发）`);
              })
            }
            title="从任务包 xlsx 手动导入回执对账"
          >
            <RefreshCw className="ic12" />
            导入回执
          </button>
        )}
      </div>

      {detail.rows.some((r) => r.status === "suspect") && (
        <div className="ban" style={{ margin: "10px 18px 0", borderColor: "var(--wr)" }}>
          <span className="f1">
            {detail.rows.filter((r) => r.status === "suspect").length} 个任务疑似已发 —
            超时无回写。绝不自动重发；请人工到平台后台核实后定态
          </span>
        </div>
      )}

      {shortages.length > 0 && (
        <div className="ban" style={{ margin: "10px 18px 0" }}>
          <span className="f1">
            缺料清单：
            {shortages
              .map((s) => `${s.code}（${shortageLabel(s.reason, s.platforms)}）`)
              .join("、")}{" "}
            — 缺料不是报错，有料 SKU 已正常出草稿
          </span>
        </div>
      )}

      {backfills.length > 0 && (
        <div className="ban" style={{ margin: "10px 18px 0" }}>
          <span className="f1">
            补排：{backfills.map((s) => s.code).join("、")} — 昨日网络超时失败，今日已自动重排
          </span>
        </div>
      )}

      <div className="f1 mt10" style={{ overflow: "auto", minHeight: 0, paddingBottom: 8 }}>
        {[...groups.entries()].map(([skuId, rows]) => {
          const head = rows[0];
          if (!head) return null;
          return (
            <div key={skuId} className="wgrp">
              <div className="wgh">
                <div
                  className="ph"
                  style={{
                    width: 34,
                    height: 34,
                    borderRadius: 7,
                    flex: "none",
                    ...bgCover(head.coverPath),
                  }}
                />
                <span className="pid">{head.skuCode}</span>
                <span className="fw5 fs12 nowrap ohide" style={{ maxWidth: 160 }}>
                  {head.styleName}
                </span>
                <span className="fs11 t3 nowrap ohide f1" style={{ minWidth: 0 }}>
                  {head.title}
                </span>
                {head.topics.slice(0, 3).map((t) => (
                  <span key={t} className="tagchip">
                    #{t}
                  </span>
                ))}
                {isDraft && (
                  <button
                    type="button"
                    className="btn sm gho"
                    onClick={() => void act(() => unwrap(commands.rerollSet(sheetId, skuId)))}
                    title="整包换该 SKU 当日套装"
                  >
                    <RefreshCw className="ic12" />
                    换套装
                  </button>
                )}
              </div>
              {rows.map((r) => {
                const st = pubTaskVisual(r.status);
                return (
                  <div key={r.id} className="wrow">
                    <span className="mono fs10 t3 nowrap">{r.taskCode.split("-")[1]}</span>
                    <span className="bdg b-gray">{r.platformZh}</span>
                    <span className="fs12 nowrap ohide">{r.accountName}</span>
                    <span className="fs11 t3 nowrap">
                      {r.contentKind === "gallery" ? "图文" : "视频"}
                    </span>
                    <span
                      className={cn("mono fs11 nowrap", isDraft && "clickable")}
                      style={isDraft ? { cursor: "pointer" } : undefined}
                      onClick={() => isDraft && setTimeEdit(r)}
                    >
                      {r.plannedTime ?? "立即发"}
                    </span>
                    <span className={cn("bdg", st.badgeClass)}>
                      {st.label}
                      {r.status === "failed" && r.failKind ? ` · ${failKindLabel(r.failKind)}` : ""}
                    </span>
                    <span
                      className="tract"
                      style={r.status === "suspect" ? { opacity: 1 } : undefined}
                    >
                      {r.status === "suspect" && (
                        <button type="button" className="btn sm" onClick={() => setVerify(r)}>
                          核对
                        </button>
                      )}
                      {isDraft && r.status === "pending" && (
                        <button
                          type="button"
                          className="btn sm gho dng"
                          onClick={() => void act(() => unwrap(commands.deleteTaskRow(r.id)))}
                        >
                          删
                        </button>
                      )}
                      {/* 已导出的单不能再删行（xlsx 已交给执行器），但可以人工取消 —— 需求 §4.5 的「已取消（人工）」态 */}
                      {isExported && r.status === "pending" && (
                        <button
                          type="button"
                          className="btn sm gho dng"
                          onClick={() => setCancelRow(r)}
                        >
                          取消
                        </button>
                      )}
                    </span>
                  </div>
                );
              })}
            </div>
          );
        })}
        {detail.rows.length === 0 && (
          <div className="bigempty" style={{ padding: "44px 20px" }}>
            <div className="fs13 fw5 t2">本单无任务行</div>
            <div className="fs12 t3">
              全部应发 SKU 缺料，或今日无应发 SKU；可「增补任务行」手动补
            </div>
          </div>
        )}
      </div>

      {/* 底部操作条 */}
      <div className="genbar">
        <span className="fs11 t3">
          {isDraft
            ? "确认后锁定进入待导出 · 导出的任务包可被影刀直接消费"
            : isConfirmed
              ? "已确认 · 可导出任务包，或退回草稿再编辑"
              : "已导出 · xlsx 写方已移交执行器"}
        </span>
        <div className="f1" />
        {isConfirmed && (
          <button
            type="button"
            className="btn sm gho"
            onClick={() => void act(() => unwrap(commands.unlockSheet(sheetId)), "已退回草稿")}
          >
            退回草稿
          </button>
        )}
        {isDraft && (
          <button
            type="button"
            className="btn pri"
            disabled={detail.rows.length === 0}
            onClick={() => void act(() => unwrap(commands.confirmSheet(sheetId)), "已确认")}
          >
            确认
          </button>
        )}
        {isConfirmed && (
          <button type="button" className="btn pri" onClick={() => setConfirmExport(true)}>
            导出任务包
          </button>
        )}
        {isExported && (
          <button
            type="button"
            className="btn sm"
            onClick={() =>
              void unwrap(commands.openPackageDir(sheetId)).catch((e) => toast.error(String(e)))
            }
          >
            <FolderOpen className="ic12" />
            打开任务包
          </button>
        )}
      </div>

      {timeEdit && (
        <TimeEditModal
          row={timeEdit}
          onClose={() => setTimeEdit(null)}
          onPick={async (time) => {
            await unwrap(commands.updateTaskRow(timeEdit.id, { plannedTime: time }));
            setTimeEdit(null);
            await load();
          }}
        />
      )}
      {addOpen && (
        <AddRowModal
          sheetId={sheetId}
          onClose={() => setAddOpen(false)}
          onAdded={() => {
            setAddOpen(false);
            void act(async () => {});
          }}
        />
      )}
      {confirmExport && (
        <ExportModal
          sheetId={sheetId}
          onClose={() => setConfirmExport(false)}
          onExported={() => {
            setConfirmExport(false);
            void act(async () => {});
          }}
        />
      )}
      {confirmRegen && (
        <ConfirmModal
          title="重新生成任务单"
          desc="这张草稿有人工调整（改过时间 / 增补过行 / 换过套装）。重新生成会按当前素材、频率与账号重算整张单，那些调整会被清掉。"
          confirmLabel="仍然重新生成"
          onConfirm={() => void regenerate()}
          onClose={() => setConfirmRegen(false)}
        />
      )}
      {cancelRow && (
        <ConfirmModal
          title="取消这条任务"
          desc={`${cancelRow.taskCode} · ${cancelRow.platformZh} · ${cancelRow.accountName}。执行器已拿到任务包，取消只改本地状态；若执行器已经发出去了，请改用「核对」。`}
          confirmLabel="确认取消"
          onConfirm={() =>
            void act(async () => {
              await unwrap(commands.cancelTaskRow(cancelRow.id));
              setCancelRow(null);
            })
          }
          onClose={() => setCancelRow(null)}
        />
      )}
      {verify && (
        <VerifyModal
          row={verify}
          onClose={() => setVerify(null)}
          onResolved={() => {
            setVerify(null);
            void act(async () => {});
          }}
        />
      )}
    </>
  );
}

/**
 * 导出弹窗：打开即跑预检，逐条列出问题。有阻断项时「确认导出」不可点——
 * 素材缺失、账号停用、执行器已回写回执，这些都必须在导出前解决，不能等到执行机上才炸。
 */
function ExportModal({
  sheetId,
  onClose,
  onExported,
}: {
  sheetId: number;
  onClose: () => void;
  onExported: () => void;
}) {
  const [report, setReport] = useState<PreflightReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

  useEffect(() => {
    void unwrap(commands.preflightExport(sheetId))
      .then(setReport)
      .catch((e) => {
        toast.error(String(e));
        onClose();
      });
  }, [sheetId, onClose]);

  // 导出期间的进度（复制视频可达数百 MB，不能只显示一个转圈）。
  useEffect(() => {
    let un: (() => void) | undefined;
    void subscribeExportProgress((e) => {
      if (e.sheetId === sheetId) setProgress({ done: e.done, total: e.total });
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, [sheetId]);

  const blocked = report == null || report.errors.length > 0;
  const doExport = async () => {
    setBusy(true);
    try {
      const r = await unwrap(commands.exportPackage(sheetId));
      toast.success(`已导出 ${r.rowCount} 行 · ${r.skuCount} 个 SKU 素材`);
      if (r.missingFiles.length > 0)
        toast.warning(`${r.missingFiles.length} 个素材文件在导出时已不存在`);
      onExported();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="导出任务包"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">导出后 xlsx 的写方移交执行器</span>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="btn sm pri"
            disabled={blocked || busy}
            onClick={() => void doExport()}
          >
            {busy
              ? progress
                ? `导出中 ${progress.done}/${progress.total}…`
                : "导出中…"
              : "确认导出"}
          </button>
        </>
      }
    >
      {report == null ? (
        <div className="fs12 t3" style={{ padding: 8 }}>
          正在预检…
        </div>
      ) : (
        <div className="col gap6" style={{ padding: 4 }}>
          {report.errors.map((e) => (
            <div key={e} className="fs12 terr" style={{ lineHeight: 1.7 }}>
              ✗ {e}
            </div>
          ))}
          {report.warnings.map((w) => (
            <div key={w} className="fs12" style={{ color: "var(--wr)", lineHeight: 1.7 }}>
              ⚠ {w}
            </div>
          ))}
          {report.errors.length === 0 && (
            <div className="fs12" style={{ color: "var(--ok)", lineHeight: 1.7 }}>
              ✓ 预检通过：{report.rowCount} 行 · {report.skuCount} 个 SKU 素材齐备
            </div>
          )}
          <div className="fs11 t3 mt6" style={{ lineHeight: 1.8 }}>
            重新导出 = 整包覆盖（先删 READY.txt 再写）。若执行器已回写过回执，
            导出会被拒绝——请先「导入回执」对账。
          </div>
        </div>
      )}
    </Modal>
  );
}

function VerifyModal({
  row,
  onClose,
  onResolved,
}: {
  row: TaskRowView;
  onClose: () => void;
  onResolved: () => void;
}) {
  const resolve = async (outcome: import("@/lib/ipc").SuspectOutcome) => {
    try {
      await unwrap(commands.resolveSuspect(row.id, outcome));
      onResolved();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <Modal
      title="核对疑似已发"
      onClose={onClose}
      headerExtra={<span className="pid">{row.taskCode}</span>}
      footer={
        <>
          <span className="fs11 t3">定态后写入使用台账</span>
          <div className="f1" />
          <button
            type="button"
            className="btn sm dng"
            onClick={() => void resolve({ kind: "failed", failKind: "other" })}
          >
            未发出 · 定为失败
          </button>
          <button
            type="button"
            className="btn pri sm"
            onClick={() => void resolve({ kind: "published", url: null })}
          >
            已发布 · 补录
          </button>
        </>
      }
    >
      <div className="susban">
        <b className="fw6">超时无回执，内容可能已实际发出。</b>
        <br />
        系统绝不自动重发 — 重发会触发平台查重，比失败更糟。请到平台后台人工核实后定态。
      </div>
      <div className="fx ac gap6 mt10 wrap">
        <span className="pid">{row.skuCode}</span>
        <span className="fs12">{row.styleName}</span>
        <span className="bdg b-gray">{row.platformZh}</span>
        <span className="chip">{row.accountName}</span>
        <span className="chip">计划 {row.plannedTime ?? "立即发"}</span>
      </div>
    </Modal>
  );
}

function bgCover(path?: string | null): React.CSSProperties {
  const src = assetSrc(path);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : { background: "var(--inset)" };
}

function TimeEditModal({
  row,
  onClose,
  onPick,
}: {
  row: TaskRowView;
  onClose: () => void;
  onPick: (time: string | null) => void | Promise<void>;
}) {
  const [slots, setSlots] = useState<string[]>([]);
  useEffect(() => {
    void unwrap(commands.getPublishSettings()).then((s) => setSlots(s.timeSlots ?? []));
  }, []);
  // 由时段模板生成候选整点/半点。
  const opts: string[] = [];
  for (const sl of slots) {
    const [a, b] = sl.split("-");
    if (!a || !b) continue;
    const [ah = 0, am = 0] = a.split(":").map(Number);
    const [bh = 0, bm = 0] = b.split(":").map(Number);
    let t = ah * 60 + am;
    const end = bh * 60 + bm;
    while (t <= end) {
      opts.push(
        `${String(Math.floor(t / 60)).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`,
      );
      t += 30;
    }
  }
  return (
    <Modal
      title="调整发布时间"
      width="w360"
      onClose={onClose}
      headerExtra={<span className="pid">{row.taskCode}</span>}
    >
      <div className="fs11 t3" style={{ marginBottom: 8 }}>
        在时段模板内选择 · 留空 = 立即发布（人工改出）
      </div>
      <div className="fx ac gap6 wrap">
        <button type="button" className="btn sm gho" onClick={() => void onPick(null)}>
          立即发
        </button>
        {opts.map((o) => (
          <button
            type="button"
            key={o}
            className={cn("btn sm", row.plannedTime === o && "pri")}
            onClick={() => void onPick(o)}
          >
            {o}
          </button>
        ))}
      </div>
    </Modal>
  );
}

function AddRowModal({
  sheetId,
  onClose,
  onAdded,
}: {
  sheetId: number;
  onClose: () => void;
  onAdded: () => void;
}) {
  const [skus, setSkus] = useState<{ id: number; code: string; styleName: string }[]>([]);
  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const [skuId, setSkuId] = useState<number | null>(null);
  const [accountId, setAccountId] = useState<number | null>(null);
  const [q, setQ] = useState("");
  useEffect(() => {
    void unwrap(commands.listSchedulableSkus()).then((v) =>
      setSkus(v.map((s) => ({ id: s.id, code: s.code, styleName: s.styleName }))),
    );
    void unwrap(commands.listAccounts()).then(setAccounts);
  }, []);
  // 100+ SKU 时长列表里靠肉眼找是不可能的（D6）。
  const filtered = useMemo(() => {
    const key = q.trim().toLowerCase();
    return skus.filter(
      (s) => !key || s.code.toLowerCase().includes(key) || s.styleName.toLowerCase().includes(key),
    );
  }, [skus, q]);
  const submit = async () => {
    if (skuId == null || accountId == null) return;
    try {
      await unwrap(commands.addTaskRow({ sheetId, skuId, accountId, plannedTime: null }));
      onAdded();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <Modal
      title="增补任务行"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">使用该 SKU 当日套装 · 时间自动分配</span>
          <div className="f1" />
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="btn pri"
            disabled={skuId == null || accountId == null}
            onClick={() => void submit()}
          >
            增补
          </button>
        </>
      }
    >
      <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
        选择 SKU
      </div>
      <input
        className="inp mt4"
        style={{ width: "100%" }}
        placeholder="搜索编码 / 款式名…"
        value={q}
        onChange={(e) => setQ(e.target.value)}
      />
      <div className="mt4" style={{ maxHeight: 260, overflow: "auto" }}>
        {filtered.map((s) => (
          <div key={s.id} className="pickrow" onClick={() => setSkuId(s.id)}>
            <span className={cn("ckb", skuId === s.id && "on")}>✓</span>
            <span className="pid">{s.code}</span>
            <span className="fs12 f1 nowrap ohide">{s.styleName}</span>
          </div>
        ))}
        {filtered.length === 0 && <div className="fs12 t3 pa8">没有匹配的 SKU</div>}
      </div>
      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
        选择账号
      </div>
      <div className="mt4" style={{ maxHeight: 140, overflow: "auto" }}>
        {accounts
          .filter((a) => a.status === "active")
          .map((a) => (
            <div key={a.id} className="pickrow" onClick={() => setAccountId(a.id)}>
              <span className={cn("ckb", accountId === a.id && "on")}>✓</span>
              <span className="bdg b-gray">{a.platformZh}</span>
              <span className="fs12 f1 nowrap ohide">{a.name}</span>
            </div>
          ))}
      </div>
    </Modal>
  );
}

// ─────────────────────────────────────────────── 策略与账号

function StrategyTab() {
  const [s, setS] = useState<PublishSettings | null>(null);
  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const [addAcct, setAddAcct] = useState(false);
  const [editAcct, setEditAcct] = useState<AccountView | null>(null);

  const load = useCallback(async () => {
    setS(await unwrap(commands.getPublishSettings()));
    setAccounts(await unwrap(commands.listAccounts()));
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const patch = async (p: Partial<PublishSettingsPatch>) => {
    try {
      const next = await unwrap(
        commands.updatePublishSettings({
          rootLocal: null,
          rootExec: null,
          pathStyle: null,
          dedupDays: null,
          receiptTimeoutHours: null,
          autogenTime: null,
          warnMaterial: null,
          warnTitle: null,
          warnBody: null,
          accountDailyLimitDefault: null,
          minGapMinutes: null,
          platformMatrix: null,
          tierRules: null,
          timeSlots: null,
          archiveRetentionDays: null,
          schedulePaused: null,
          ...p,
        }),
      );
      setS(next);
    } catch (e) {
      // 后端拒绝（如非法时段）→ 提示并回读，避免界面停在没存进去的值上。
      toast.error(String(e));
      await load();
    }
  };

  if (!s) return <div className="col f1" />;
  const tr = s.tierRules ?? { hotDaily: 1, warmWeekly: 3, coldWeeklyRotate: 5 };
  const m = s.platformMatrix ?? {
    douyin: true,
    xhs: true,
    kuaishou: true,
    shipinhao: true,
    bilibili: true,
  };
  const timeSlots = s.timeSlots ?? [];
  const plats: [keyof typeof m, string][] = [
    ["douyin", "抖音"],
    ["xhs", "小红书"],
    ["kuaishou", "快手"],
    ["shipinhao", "视频号"],
    ["bilibili", "B站"],
  ];

  return (
    <div className="pbody">
      <div className="cwrap">
        <div className="card" style={{ padding: "13px 16px" }}>
          <div className="fw6 fs13" style={{ marginBottom: 10 }}>
            分层频率
          </div>
          <div className="fx gap14 wrap">
            {/* 引擎语义是「热款每天发一次 × 平台集」，>1 无效 —— 用开关而不是数字框，
                免得调到 3 却什么也没发生。同 SKU 同日多套装留给 V2。 */}
            <div className="fx ac gap8">
              <span className="fs12 t2 nowrap">热款每日发布</span>
              <Toggle
                on={tr.hotDaily >= 1}
                onClick={() =>
                  void patch({ tierRules: { ...tr, hotDaily: tr.hotDaily >= 1 ? 0 : 1 } })
                }
              />
              <span className="fs11 t3">每天一次 × 平台集</span>
            </div>
            <div className="fx ac gap8">
              <span className="fs12 t2 nowrap">温款每周</span>
              <Stepper
                value={tr.warmWeekly}
                min={0}
                max={7}
                onChange={(v) => void patch({ tierRules: { ...tr, warmWeekly: v } })}
              />
              <span className="fs11 t3">次</span>
            </div>
            <div className="fx ac gap8">
              <span className="fs12 t2 nowrap">冷款每周轮出</span>
              <Stepper
                value={tr.coldWeeklyRotate}
                min={0}
                max={20}
                onChange={(v) => void patch({ tierRules: { ...tr, coldWeeklyRotate: v } })}
              />
              <span className="fs11 t3">个</span>
            </div>
          </div>
        </div>

        <div className="card mt14" style={{ padding: "13px 16px" }}>
          <div className="fw6 fs13" style={{ marginBottom: 10 }}>
            平台矩阵（全局启用）
          </div>
          <div className="fx gap14 wrap">
            {plats.map(([k, label]) => (
              <div key={k} className="fx ac gap6">
                <Toggle
                  on={m[k]}
                  onClick={() => void patch({ platformMatrix: { ...m, [k]: !m[k] } })}
                />
                <span className="fs12">{label}</span>
              </div>
            ))}
          </div>
        </div>

        <div className="card mt14">
          <div className="chead">
            <span className="fw6 fs13">账号档案</span>
            <span className="cnt">{accounts.length}</span>
            <div className="f1" />
            <button type="button" className="btn sm" onClick={() => setAddAcct(true)}>
              <Plus className="ic12" />
              添加账号
            </button>
          </div>
          {accounts.map((a) => (
            <div key={a.id} className="txrow">
              <span className="bdg b-gray">{a.platformZh}</span>
              <span className="fw5 fs12 f1 nowrap ohide">{a.name}</span>
              <span className="fs11 t3 nowrap">日限 {a.dailyLimit}</span>
              {a.slots && a.slots.length > 0 && (
                <span className="chip" title="该账号的可用时段（覆盖全局模板）">
                  {a.slots.join(" ")}
                </span>
              )}
              <span className={cn("bdg", a.status === "active" ? "b-green" : "b-gray")}>
                {a.status === "active" ? "正常" : "停用"}
              </span>
              <span className="tract">
                <button type="button" className="btn sm gho" onClick={() => setEditAcct(a)}>
                  编辑
                </button>
                <button
                  type="button"
                  className="btn sm gho"
                  onClick={() =>
                    void unwrap(
                      commands.setAccountStatus(
                        a.id,
                        a.status === "active" ? "disabled" : "active",
                      ),
                    ).then(load)
                  }
                >
                  {a.status === "active" ? "停用" : "启用"}
                </button>
              </span>
            </div>
          ))}
          {accounts.length === 0 && (
            <div className="txrow t3 fs12">尚无账号 · 添加后才能生成任务行</div>
          )}
        </div>

        <SlotsCard
          slots={timeSlots}
          minGap={s.minGapMinutes ?? 60}
          onSave={(next) => patch({ timeSlots: next })}
        />
      </div>

      {addAcct && (
        <AddAccountModal
          onClose={() => setAddAcct(false)}
          onAdded={() => {
            setAddAcct(false);
            void load();
          }}
        />
      )}
      {editAcct && (
        <EditAccountModal
          account={editAcct}
          onClose={() => setEditAcct(null)}
          onSaved={() => {
            setEditAcct(null);
            void load();
          }}
        />
      )}
    </div>
  );
}

/** `HH:MM-HH:MM`，开始早于结束（与后端 `scheduler::parse_slot` 同一套规则）。 */
function parseSlotErr(v: string): string | null {
  const m = /^(\d{1,2}):(\d{2})\s*-\s*(\d{1,2}):(\d{2})$/.exec(v.trim());
  if (!m) return "格式应为 HH:MM-HH:MM";
  const [h1, m1, h2, m2] = m.slice(1).map(Number) as [number, number, number, number];
  const ok = (h: number, mi: number) => h >= 0 && h < 24 && mi >= 0 && mi < 60;
  if (!ok(h1, m1) || !ok(h2, m2)) return "时间超出范围（00:00–23:59）";
  const a = h1 * 60 + m1;
  const b = h2 * 60 + m2;
  if (a >= b) return "暂不支持跨午夜时段，请拆成 21:00-23:59";
  return null;
}

/** 时段模板编辑：chips 可删，回车新增。空列表 = 全部立即发（保存时明确警告）。 */
function SlotsCard({
  slots,
  minGap,
  onSave,
}: {
  slots: string[];
  minGap: number;
  onSave: (next: string[]) => Promise<void> | void;
}) {
  const [draft, setDraft] = useState("");
  const err = draft.trim() ? parseSlotErr(draft) : null;

  const add = async () => {
    const v = draft.trim();
    if (!v || parseSlotErr(v)) return;
    if (slots.includes(v)) {
      setDraft("");
      return;
    }
    await onSave([...slots, v].sort());
    setDraft("");
  };
  const remove = async (slot: string) => {
    const next = slots.filter((s) => s !== slot);
    if (next.length === 0) toast.warning("时段模板已清空：所有任务将变成「立即发」");
    await onSave(next);
  };

  return (
    <div className="card mt14" style={{ padding: "13px 16px" }}>
      <div className="fw6 fs13" style={{ marginBottom: 8 }}>
        时段模板
      </div>
      <div className="fx ac gap6 wrap">
        {slots.map((slot) => (
          <span key={slot} className="fchip">
            {slot}
            <button
              type="button"
              className="icb"
              style={{ width: 16, height: 16, marginLeft: 4 }}
              aria-label={`删除时段 ${slot}`}
              onClick={() => void remove(slot)}
            >
              ×
            </button>
          </span>
        ))}
        <input
          className="inp mono"
          style={{ width: 130, ...(err ? { borderColor: "var(--er)" } : {}) }}
          placeholder="09:00-11:00"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void add()}
        />
        <button
          type="button"
          className="btn sm"
          disabled={!draft.trim() || !!err}
          onClick={() => void add()}
        >
          添加时段
        </button>
      </div>
      <div className={cn("fs11 mt6", err ? "terr" : "t3")}>
        {err ??
          (slots.length === 0
            ? "时段为空 → 所有任务立即发（不定时）"
            : `分配时在时段内随机抖动 · 同平台多账号最小间隔 ${minGap} 分钟`)}
      </div>
    </div>
  );
}

/** 账号编辑：改名 / 日限 / 可用时段（覆盖全局模板）/ 删除。 */
function EditAccountModal({
  account,
  onClose,
  onSaved,
}: {
  account: AccountView;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [name, setName] = useState(account.name);
  const [limit, setLimit] = useState(account.dailyLimit);
  const [slots, setSlots] = useState<string[]>(account.slots ?? []);
  const [draft, setDraft] = useState("");
  const [confirmDel, setConfirmDel] = useState(false);
  const err = draft.trim() ? parseSlotErr(draft) : null;

  const addSlot = () => {
    const v = draft.trim();
    if (!v || parseSlotErr(v) || slots.includes(v)) return;
    setSlots([...slots, v].sort());
    setDraft("");
  };
  const save = async () => {
    try {
      await unwrap(
        commands.updateAccount(account.id, {
          name: name.trim() || null,
          dailyLimit: limit,
          // 空数组 = 清除覆盖，跟随全局时段模板。
          slots: slots.length > 0 ? slots : null,
        }),
      );
      onSaved();
    } catch (e) {
      toast.error(String(e));
    }
  };
  const del = async () => {
    try {
      await unwrap(commands.deleteAccount(account.id));
      toast.success("账号已删除");
      onSaved();
    } catch (e) {
      toast.error(String(e));
      setConfirmDel(false);
    }
  };

  return (
    <>
      <Modal
        title="编辑账号"
        onClose={onClose}
        headerExtra={<span className="bdg b-gray">{account.platformZh}</span>}
        footer={
          <>
            <button type="button" className="btn sm gho dng" onClick={() => setConfirmDel(true)}>
              删除
            </button>
            <div className="f1" />
            <button type="button" className="btn sm" onClick={onClose}>
              取消
            </button>
            <button type="button" className="btn sm pri" onClick={() => void save()}>
              保存
            </button>
          </>
        }
      >
        <div className="col gap10">
          <div className="col gap4">
            <span className="fs11 t3">账号名称（任务单「平台账号名称」列的值）</span>
            <input className="inp" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="fx ac gap8">
            <span className="fs12 t2 nowrap">日发布上限</span>
            <Stepper value={limit} min={1} max={100} onChange={setLimit} />
            <span className="fs11 t3">条</span>
          </div>
          <div className="col gap4">
            <span className="fs11 t3">可用时段（留空 = 跟随全局时段模板）</span>
            <div className="fx ac gap6 wrap">
              {slots.map((slot) => (
                <span key={slot} className="fchip">
                  {slot}
                  <button
                    type="button"
                    className="icb"
                    style={{ width: 16, height: 16, marginLeft: 4 }}
                    aria-label={`删除时段 ${slot}`}
                    onClick={() => setSlots(slots.filter((s) => s !== slot))}
                  >
                    ×
                  </button>
                </span>
              ))}
              <input
                className="inp mono"
                style={{ width: 130, ...(err ? { borderColor: "var(--er)" } : {}) }}
                placeholder="09:00-11:00"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && addSlot()}
              />
            </div>
            {err && <span className="fs11 terr">{err}</span>}
          </div>
        </div>
      </Modal>
      {confirmDel && (
        <ConfirmModal
          title="删除账号"
          desc={`删除「${account.name}」。有历史任务的账号删不掉（会让历史记录失去归属），请改用「停用」。`}
          confirmLabel="确认删除"
          onConfirm={() => void del()}
          onClose={() => setConfirmDel(false)}
        />
      )}
    </>
  );
}

function AddAccountModal({ onClose, onAdded }: { onClose: () => void; onAdded: () => void }) {
  const [platforms, setPlatforms] = useState<{ code: string; zh: string }[]>([]);
  const [platform, setPlatform] = useState("");
  const [name, setName] = useState("");
  useEffect(() => {
    void commands.publishPlatforms().then((r) => {
      if (r.status === "ok") {
        setPlatforms(r.data);
        setPlatform(r.data[0]?.code ?? "");
      }
    });
  }, []);
  const submit = async () => {
    if (!platform || !name.trim()) return;
    try {
      await unwrap(
        commands.createAccount({ platform, name: name.trim(), dailyLimit: null, slots: null }),
      );
      onAdded();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <Modal
      title="添加账号"
      onClose={onClose}
      footer={
        <>
          <div className="f1" />
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn pri" onClick={() => void submit()}>
            添加
          </button>
        </>
      }
    >
      <div className="col gap10">
        <div className="col gap4">
          <span className="fs11 t3">平台</span>
          <select className="inp" value={platform} onChange={(e) => setPlatform(e.target.value)}>
            {platforms.map((p) => (
              <option key={p.code} value={p.code}>
                {p.zh}
              </option>
            ))}
          </select>
        </div>
        <div className="col gap4">
          <span className="fs11 t3">账号名称（任务单「平台账号名称」列的值）</span>
          <input className="inp" value={name} onChange={(e) => setName(e.target.value)} />
        </div>
      </div>
    </Modal>
  );
}
