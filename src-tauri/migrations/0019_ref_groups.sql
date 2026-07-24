-- 参考图库自有分组 + 生成页临时上传位。forward-only。
--
-- 起因是两条互相纠缠的错配：
--
-- 1) `ref_images.group_id` 指向 **prompt_groups**——参考图库的分组名一直借用提示词组的名字。
--    提示词组是「一份 txt = 一个组」的产物，随导入不断新增、还有临时组，
--    拿它当长期图库的目录，等于让图库的架子跟着别人的节奏变形。
--    本条建 `ref_groups` 作为图库自己的分组表，并把既有归属**按同名搬过去**
--    （不是丢进未分组）：用户眼前的组织结构一张不动，只是链子断了。
--    `group_id` 列保留不动（历史列，仅 0001 的 FK 还挂着），新代码一律不读不写。
--
-- 2) 生成页「上传」的图和图库「导入」的图走同一条命令，于是随手拖进来跑一次的图
--    永久占着长期图库。加 `ephemeral` 位：生成页上传的图只作本批附件，
--    仍是 ref_images 行（tasks/batch_refs/accepted_works 都以它为父表，不能不落库），
--    但图库页与「从参考图库选择」都不列它，去重扫描也不拿它当基准。

CREATE TABLE ref_groups (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL,
  -- 手工排序位（越小越靠前）；同值按 id。
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
-- 大小写不敏感唯一：避免「产品图」与「产品圖」式的肉眼重名（同 0012 对 SKU 编码的处理）。
CREATE UNIQUE INDEX idx_ref_groups_name ON ref_groups (name COLLATE NOCASE);

ALTER TABLE ref_images ADD COLUMN ref_group_id INTEGER REFERENCES ref_groups (id) ON DELETE SET NULL;
ALTER TABLE ref_images ADD COLUMN ephemeral INTEGER NOT NULL DEFAULT 0 CHECK (ephemeral IN (0, 1));
CREATE INDEX idx_ref_images_ref_group ON ref_images (ref_group_id);
CREATE INDEX idx_ref_images_ephemeral ON ref_images (ephemeral);

-- 搬运既有归属：为每个「实际被参考图用到的」提示词组名建一个同名图库分组。
-- OR IGNORE 兜住 NOCASE 撞名（两个提示词组只差大小写时合并为一个图库分组）。
INSERT OR IGNORE INTO ref_groups (name, sort_order, created_at)
SELECT DISTINCT pg.name, 0, strftime('%s', 'now')
FROM ref_images ri
JOIN prompt_groups pg ON pg.id = ri.group_id
WHERE ri.deleted_at IS NULL;

-- prompt_groups.name 无唯一约束（0001），同名组可并存 → 子查询必须 LIMIT 1，
-- 否则「一行多值」会让整条迁移报错。同名本就该落到同一个图库分组，取谁都一样。
UPDATE ref_images
SET ref_group_id = (
  SELECT rg.id FROM ref_groups rg
  WHERE rg.name = (SELECT pg.name FROM prompt_groups pg WHERE pg.id = ref_images.group_id)
    COLLATE NOCASE
  LIMIT 1
)
WHERE group_id IS NOT NULL AND deleted_at IS NULL;
