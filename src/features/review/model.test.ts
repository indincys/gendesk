import type { ReviewItemView } from "@/lib/ipc";
import { describe, expect, it } from "vitest";
import { buildSections, itemLabel, refLabel } from "./model";

const now = 1_790_000_000;
const generatedId = `cli-${"a".repeat(32)}`;
const organization = { product: "明星日历", style: "款式甲", requirement: "桌面场景", source: "" };

function image(id: number, fields: Partial<ReviewItemView> = {}): ReviewItemView {
  return {
    id,
    batchId: id,
    batchNote: generatedId,
    refId: id,
    refName: "b".repeat(64),
    promptCode: `P-${id}`,
    itemName: "",
    groupId: id,
    groupName: generatedId,
    organization: null,
    keyAlias: null,
    resultImagePath: null,
    resultThumbPath: null,
    promptText: "",
    refThumbPath: null,
    refImagePath: null,
    resultWidth: null,
    resultHeight: null,
    createdAt: now,
    skill: null,
    ...fields,
  };
}

describe("图片验收分组", () => {
  it("跨提交合并同一款式的同一需求，分开不同款式和需求", () => {
    const sections = buildSections(
      [
        image(1, { organization }),
        image(2, { organization }),
        image(3, { organization: { ...organization, style: "款式乙" } }),
        image(4, { organization: { ...organization, requirement: "质量验证" } }),
      ],
      "requirement",
      now,
    );
    expect(sections.map(({ items }) => items.map(({ id }) => id))).toEqual([[4], [2, 1], [3]]);
    expect(sections[1]?.heads[0]).toMatchObject({
      name: "桌面场景 · 款式甲",
      detail: "明星日历 · 2 次提交",
      ids: [2, 1],
    });
    const remaining = sections.flatMap(({ items }) => items).filter(({ id }) => id !== 2);
    expect(buildSections(remaining, "requirement", now).map(({ key }) => key)).toEqual(
      sections.map(({ key }) => key),
    );
  });

  it("同名提交分别处理，历史工单标识只用作追溯", () => {
    const items = [
      image(1, { batchNote: "桌景返工" }),
      image(2, { batchNote: "桌景返工" }),
      image(3),
    ];
    const sections = buildSections(items, "batch", now);
    expect(sections.map(({ heads }) => heads[0])).toMatchObject([
      { name: "未命名提交", ids: [3] },
      { name: "桌景返工", ids: [2] },
      { name: "桌景返工", ids: [1] },
    ]);
    expect(buildSections(items, "requirement", now)).toHaveLength(3);
  });

  it("卡片优先显示条目名称，参考图内容哈希用编号代替", () => {
    const item = image(5, { itemName: "桌景第五张" });
    expect(itemLabel(item)).toBe("桌景第五张");
    expect(refLabel(item)).toBe("参考图 #5");
  });
});
