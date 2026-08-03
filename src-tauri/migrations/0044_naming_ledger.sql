-- 0044：命名与发号台账。forward-only。
--
-- 图片的身份从此**写在文件名上**：`W-王一博-55-正面-hu72.jpg`
--   · `W-王一博-55-正面` 语义名（含 SKU 数字缩写 55）—— skill 给的，GenDesk 一个字符都不改
--   · `hu72`             4 位随机 UID          —— GenDesk 给的，验收通过那一刻发放
--
-- 下游（`自动发布`）永远读交付目录里的 manifest，**不解析文件名**；文件名只是人工
-- 核对的双保险。两边不各存一份对照表：发号处只有 GenDesk 一个（分叉的代价见
-- `channel_of` / `CHANNEL_OF` 那次——设置页显示 50、真跑的信号量是 10）。

-- ── 1. 发号台账：只增不删 ────────────────────────────────────────────────
--
-- **不设外键、不随作品删除而消失。** 作品是本仓库唯一「删除即真删行」的实体
-- （accepted_works 没有 deleted_at，靠 trash_items.payload_json 整行还原）。作品一删，
-- uid 就失忆了，而那个文件很可能早已交付到 `自动发布` 那边、正被别人当主键用着。
-- 这张表只回答一个问题：**这个名字 / 这个 UID 历史上发出去过没有**。
-- 设计理由与 0018 work_exports 完全一致。
--
-- work_id 可空且允许孤儿：UID 必须在**拷贝之前**就定下来（文件名里有它），而库里的
-- 作品行要等整批拷完才写（0027 之后一直如此：拷贝无法回滚，单事务反而更糟）。
-- 中途失败会留下一个没有主人的号 —— 那正是「不回收」的正常代价，比重复发号便宜得多。
CREATE TABLE issued_names (
  -- 完整文件名主干（不含扩展名）：W-王一博-55-正面-hu72
  name      TEXT PRIMARY KEY,
  -- 4 位随机 UID，全局唯一（不按前缀分域），这样 `自动发布` 可以只存 uid 当主键。
  uid       TEXT NOT NULL,
  work_id   INTEGER,
  issued_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_issued_names_uid ON issued_names (uid);

-- ── 2. 作品的命名与 SKU 快照 ────────────────────────────────────────────
--
-- 理由与 0027 给 prompt_code / group_name 存快照**完全一致**：提示词是消耗品，
-- 批次跑完即删；而人隔几天才回来验收是常态。不冗余的话，manifest 到那时就写不出
-- SKU 了 —— 而分错 SKU 目录是**静默失败**（不报错，只是放错地方）。
ALTER TABLE accepted_works ADD COLUMN item_name TEXT NOT NULL DEFAULT ''; -- W-王一博-55-正面
ALTER TABLE accepted_works ADD COLUMN uid       TEXT NOT NULL DEFAULT ''; -- hu72
-- SKU 数字缩写（`55`），由 skill 在工单里给定。
-- **与 accepted_works.sku_id / skus.code 不是一回事**：那两个属于 publish 子系统
-- （素材包 / 任务单），已停止投入；这一列是交付给 `自动发布` 的归类键。
ALTER TABLE accepted_works ADD COLUMN sku_code  TEXT NOT NULL DEFAULT ''; -- 55
ALTER TABLE accepted_works ADD COLUMN style     TEXT NOT NULL DEFAULT ''; -- 王一博
-- 受控用途（当前只有「图生视频」）。载体从 tag_bindings 换到这里的理由见 0045。
ALTER TABLE accepted_works ADD COLUMN purpose   TEXT NOT NULL DEFAULT '';

-- ── 3. 提示词是这几样东西的载体 ─────────────────────────────────────────
--
-- 语义名 / SKU / 风格 / 用途都是**条目级**的（工单 v2 里一个 item = 一条提示词
-- = 一个任务 = 一张图），故它们跟着提示词走，验收那一刻快照进 accepted_works。
--
-- 为什么是 prompts 而不是 tasks：同一条词可以被再跑一次（手动重跑仍是那条词写的图），
-- 而任务只是「这一次拿它跑了一遍」。这与 0032 把 skill 落在 prompts 上是同一个判断。
--
-- 空串 = 不知道（生成页手动建的批、0044 之前的历史数据）。下游对空串走兜底语义名，
-- **不猜**：从中文文件名反推 SKU 会静默分错目录（`王一博` / `王一博 ` / `王一博A`）。
ALTER TABLE prompts ADD COLUMN item_name TEXT NOT NULL DEFAULT '';
ALTER TABLE prompts ADD COLUMN sku_code  TEXT NOT NULL DEFAULT '';
ALTER TABLE prompts ADD COLUMN style     TEXT NOT NULL DEFAULT '';
ALTER TABLE prompts ADD COLUMN purpose   TEXT NOT NULL DEFAULT '';
