-- SKU 文件夹别名：收件箱子文件夹用中文/自定义命名（如 A-敖瑞鹏-01）时映射到 SKU 编码。
-- 一对一：每个 SKU 存一条别名（可空）；收录识别时 code 未命中再按别名查库。
-- 别名允许中文（绕开 SKU 编码的 ASCII 限制），故不走 is_valid_sku_code 校验。
ALTER TABLE skus ADD COLUMN folder_alias TEXT NOT NULL DEFAULT '';

-- 非空别名全局唯一（一个别名只能指向一个 SKU）；空串不参与唯一约束（多 SKU 可无别名）。
CREATE UNIQUE INDEX idx_skus_folder_alias ON skus(folder_alias) WHERE folder_alias != '';
