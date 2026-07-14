/** 发布计划页（P2/P3 实现看板/任务单/策略与账号三页签）。P1 先占位路由。 */
export function PlanPage() {
  return (
    <div className="col f1 ohide">
      <div className="phd">
        <span className="ptt">发布计划</span>
        <span className="pcap">任务单编排 · 回执对账 · 看板日报</span>
      </div>
      <div className="bigempty" style={{ padding: "72px 20px" }}>
        <div className="fs13 fw5 t2">发布计划将在编排与导出阶段上线</div>
        <div className="fs12 t3">
          先在设置页配置根目录，并在资产库积累 SKU 与素材；任务单编排、导出与回执对账随后接入
        </div>
      </div>
    </div>
  );
}
