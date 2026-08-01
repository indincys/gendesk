-- RelPath 是图片素材库的存储契约。0036 已在真实库应用，不可修改其校验和，
-- 因此用独立 forward-only 迁移为新增与更新建立数据库级守卫。
CREATE TRIGGER trg_image_assets_relpath_insert
BEFORE INSERT ON image_assets
WHEN NEW.path_rel LIKE '/%'
  OR substr(NEW.path_rel, 1, 1) = char(92)
  OR instr(NEW.path_rel, ':') > 0
  OR NEW.thumb_rel LIKE '/%'
  OR substr(NEW.thumb_rel, 1, 1) = char(92)
  OR instr(NEW.thumb_rel, ':') > 0
BEGIN
  SELECT RAISE(ABORT, 'image_assets paths must be relative');
END;

CREATE TRIGGER trg_image_assets_relpath_update
BEFORE UPDATE OF path_rel, thumb_rel ON image_assets
WHEN NEW.path_rel LIKE '/%'
  OR substr(NEW.path_rel, 1, 1) = char(92)
  OR instr(NEW.path_rel, ':') > 0
  OR NEW.thumb_rel LIKE '/%'
  OR substr(NEW.thumb_rel, 1, 1) = char(92)
  OR instr(NEW.thumb_rel, ':') > 0
BEGIN
  SELECT RAISE(ABORT, 'image_assets paths must be relative');
END;
