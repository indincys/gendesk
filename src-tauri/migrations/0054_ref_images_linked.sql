-- 参考图库改为映射本地原图（不再把原图拷进数据目录）。
-- linked = 1：file_path 是用户自己的文件，应用只读不写——不移动、不删除、不在它旁边写任何东西。
-- 缩略图与（需要时的）摆正/压缩上传副本仍写在数据目录里。
-- 只加一列带默认值的列，旧版本应用读写不受影响（旧行一律是拷贝进来的，默认 0 正确）。
ALTER TABLE ref_images ADD COLUMN linked INTEGER NOT NULL DEFAULT 0;
