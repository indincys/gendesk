import { V2vVideo } from "@/features/v2v/V2vVideo";
import { type Row, STAGE_META } from "@/features/v2v/model";
import { assetSrc } from "@/lib/img";
import { cn } from "@/lib/utils";

/**
 * 工作台左栏 —— 这一条长什么样。
 *
 * ## 为什么它值一整栏
 *
 * v0.24.0 之前这块画面挤在 268px 的详情栏顶端，而这一页真正费眼睛的事恰恰是看片
 * （判的是色差与形变）。主轴筛选搬进侧栏之后腾出来的地方全给了它。
 *
 * ## 没有成片时显示**首帧原图**，不摆空占位
 *
 * 缺词 / 就绪 / 远端这几个阶段，人在这一栏要做的事都要看那张图：判断改写出来的
 * 运镜配不配得上它、决定要不要放行。一块「尚无成片」的灰板既没有信息，又占掉整栏。
 *
 * ## 读的是 `imagePath` 而不是 `thumbPath`
 *
 * `accepted_works.thumb_path` 是长边 512px、JPEG q80 的缩略图 —— 它是给一屏几十格的
 * 网格用的，放进这一整栏就是一张糊图，而这一栏存在的全部理由是**看清细节**
 * （判色差与形变）。`image_path` 是这张图的原始像素，同一张图，只是没被缩过。
 *
 * 它与生成质量无关：提交给即梦的一直是 `clip.image_path`（`runner` → `dreamina::submit`
 * 的 `--image=`），从来不是缩略图。这里换的只是**显示**读哪一份。
 */
export function V2vPreview({
  row,
  index,
  total,
  showFrame,
  onToggleFrame,
}: {
  row: Row | null;
  /** 在当前这一屏里排第几（从 1 起）。0 = 没有选中。 */
  index: number;
  total: number;
  showFrame: boolean;
  onToggleFrame: (v: boolean) => void;
}) {
  if (!row) {
    return (
      <div className="vpv">
        <div className="vpvhd">
          <span className="t3 fs12">没有选中的条目</span>
        </div>
        <div className="vpvbody">
          <span className="vpvnote">左边选一档，右边点一条</span>
        </div>
      </div>
    );
  }

  const c = row.clip;
  const meta = STAGE_META[row.stage];
  const video = assetSrc(c.videoPath);
  // 原图，不是缩略图（见文件头）。
  const frame = assetSrc(c.imagePath);
  // 有片子时默认放片子（那时要判的是「动起来之后还对不对」）；没片子就只能是首帧。
  const asFrame = (showFrame || !video) && !!frame;

  return (
    <div className="vpv">
      <div className="vpvhd">
        <span className="pid big">{c.promptCode}</span>
        <span className="vstg" style={{ background: meta.bg, color: meta.fg }}>
          <span className="d" />
          {meta.label}
        </span>
        <div className="f1" />
        <span className="fs11 t3 nowrap">{total === 0 ? "空" : `${index} / ${total}`}</span>
        <div className="seg">
          <span
            className={cn("sgi", !asFrame && "on", !video && "dis")}
            onClick={() => video && onToggleFrame(false)}
          >
            成片
          </span>
          <span
            className={cn("sgi", asFrame && "on", !frame && "dis")}
            onClick={() => frame && onToggleFrame(true)}
          >
            首帧
          </span>
        </div>
      </div>

      <div className="vpvbody">
        {asFrame ? (
          <div className="vpvimg">
            <img src={frame} alt="首帧原图" />
          </div>
        ) : video ? (
          // 循环 + 静音自动播放：验收判的是「动起来之后还对不对」，
          // 每条都要手点播放会让一次 46 条的验收多出 46 次点击。
          <V2vVideo src={video} fps={c.fps} dark videoKey={c.id} className="f1" />
        ) : (
          <div className="vpvempty">
            <span className="vpvnote">{row.stage === "run" ? "还没有成片" : "还没有成片"}</span>
            <span className="vpvhint">
              {/* 这句要答「所以现在是什么状况」，而那句话 `situation` 已经算好了 ——
                  在这儿另写一套，两处迟早会对不上。 */}
              {row.situation}
            </span>
          </div>
        )}
        <div className="vpvmeta">
          <span className="mono">
            {c.width != null && `${c.width}×${c.height}`}
            {c.durationSec != null && ` · ${c.durationSec.toFixed(1)}s`}
            {c.fps != null && ` · ${Math.round(c.fps)}fps`}
          </span>
          <div className="f1" />
        </div>
      </div>
    </div>
  );
}
