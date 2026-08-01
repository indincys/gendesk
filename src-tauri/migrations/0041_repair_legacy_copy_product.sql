-- 修复 0037 在 SKU 尚未归属商品时无法完成的存量文案回填。
UPDATE text_items
SET product_id = (SELECT product_id FROM skus WHERE skus.id = text_items.sku_id)
WHERE product_id IS NULL
  AND sku_id IS NOT NULL
  AND EXISTS(
    SELECT 1 FROM skus WHERE skus.id = text_items.sku_id AND skus.product_id IS NOT NULL
  );

-- 后续在目录导入或批量指派时，让仍保留旧 sku_id 的 free 文案跟随 SKU 归属。
CREATE TRIGGER sync_free_copy_product_after_sku_reassign
AFTER UPDATE OF product_id ON skus
WHEN OLD.product_id IS NOT NEW.product_id
BEGIN
  UPDATE text_items
  SET product_id = NEW.product_id
  WHERE sku_id = NEW.id AND state = 'free';
END;
