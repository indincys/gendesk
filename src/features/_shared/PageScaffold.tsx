import type { ReactNode } from "react";

/**
 * 页面骨架（M0）—— 复用原型 `.phd` 页头与主体容器。
 * M3 各页实现时逐页替换 `children` 为真实内容。
 */
export function PageScaffold({
  title,
  caption,
  right,
  children,
  bodyClassName,
}: {
  title: string;
  caption?: string;
  right?: ReactNode;
  children?: ReactNode;
  /** 页面主体需要自管滚动/布局时附加；默认仍由 `.pbody` 滚动。 */
  bodyClassName?: string;
}) {
  return (
    <div className="col f1 ohide" data-screen-label={title}>
      <div className="phd">
        <span className="ptt">{title}</span>
        {caption && <span className="pcap">{caption}</span>}
        <div className="f1" />
        {right}
      </div>
      <div className={bodyClassName ? `pbody ${bodyClassName}` : "pbody"}>
        {children ?? <MilestoneNote page={title} />}
      </div>
    </div>
  );
}

function MilestoneNote({ page }: { page: string }) {
  return (
    <div className="bigempty">
      <div className="fs13 fw5 t2">{page} · 骨架就绪</div>
      <div className="fs12 t3">页面外壳与设计 tokens 已落地；完整交互将在 M3 按原型实现。</div>
    </div>
  );
}
