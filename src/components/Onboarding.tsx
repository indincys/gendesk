import { commands, unwrap } from "@/lib/ipc";
import type { RouteKey } from "@/routes";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { Check } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

interface Step {
  key: string;
  label: string;
  hint: string;
  route: RouteKey;
  done: boolean;
}

/**
 * 首次使用引导（E13）：资产全空 / 未完成引导时显示四步清单，
 * 每步完成自动勾选、可点击跳转；四步齐备后置 onboarded 并永久消失。
 */
export function Onboarding() {
  const settings = useSettingsStore((s) => s.settings);
  const update = useSettingsStore((s) => s.update);
  const go = useUiStore((s) => s.go);
  const route = useUiStore((s) => s.route);
  const [steps, setSteps] = useState<Step[] | null>(null);

  const check = useCallback(async () => {
    try {
      const [keys, groups, refs, tasks] = await Promise.all([
        unwrap(commands.listApiKeys()).catch(() => []),
        unwrap(commands.listPromptGroups()).catch(() => []),
        // 不含临时上传：在生成页随手拖一张试跑，不等于「参考图库里有素材」。
        unwrap(commands.listRefImages(false)).catch(() => []),
        // 批次跑完就退出历史（v0.21.0），故「跑过第一批没有」不能再靠批次列表回答。
        // 任务同样会随批次消失，但作品不会 —— 出过一张图就算跑过。
        unwrap(
          commands.listWorks(
            {
              groupId: null,
              favoriteOnly: false,
              tag: null,
              query: null,
              batchId: null,
            },
            0,
          ),
        ).catch(() => []),
      ]);
      setSteps([
        {
          key: "key",
          label: "配置 API Key",
          hint: "到设置添加至少一个可用的生图 Key",
          route: "settings",
          done: keys.length > 0,
        },
        {
          key: "prompts",
          label: "导入提示词",
          hint: "在生成页导入 .txt，或让 skill 投一份工单进来",
          route: "generate",
          done: groups.some((g) => g.count > 0),
        },
        {
          key: "refs",
          label: "上传参考图",
          hint: "在参考图库上传要生成的素材图",
          route: "refs",
          done: refs.length > 0,
        },
        {
          key: "generate",
          label: "开始生成",
          hint: "在生成页挂靠组合并开始第一批",
          route: "generate",
          done: tasks.length > 0,
        },
      ]);
    } catch {
      // 引导为可选提示，失败静默
    }
  }, []);

  // 未完成引导时：挂载 + 每次切页回来重新校验各步完成度。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖 route 触发重查
  useEffect(() => {
    if (settings && !settings.onboarded) void check();
  }, [settings?.onboarded, route, check]);

  // 四步齐备 → 持久化完成，永久消失。
  useEffect(() => {
    if (steps?.every((s) => s.done) && settings && !settings.onboarded) {
      void update({ onboarded: true });
    }
  }, [steps, settings, update]);

  if (!settings || settings.onboarded || !steps) return null;

  return (
    <div className="onbwrap">
      <div className="onbcard">
        <div className="fx ac gap8">
          <span className="fw6 fs14 f1">欢迎使用 GenDesk · 四步开始批量生产</span>
          <button
            type="button"
            className="fs11 t3"
            onClick={() => void update({ onboarded: true })}
            title="跳过引导（可在完成任一步后消失）"
          >
            跳过
          </button>
        </div>
        <div className="col mt10" style={{ gap: 8 }}>
          {steps.map((s, i) => (
            <button key={s.key} type="button" className="onbstep" onClick={() => go(s.route)}>
              <span className={s.done ? "onbtick done" : "onbtick"}>
                {s.done ? <Check className="ic12" /> : i + 1}
              </span>
              <span className="col f1" style={{ gap: 2, minWidth: 0, textAlign: "left" }}>
                <span className="fs12 fw5">{s.label}</span>
                <span className="fs11 t3">{s.hint}</span>
              </span>
              <span className="fs11 t3">{s.done ? "已完成" : "去设置 ›"}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
