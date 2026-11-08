-- 内容归属独立于运行批次；作品长期保存，不能随任务退休丢掉归属。
ALTER TABLE accepted_works ADD COLUMN organization_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE v2v_clips ADD COLUMN organization_json TEXT NOT NULL DEFAULT '{}';
UPDATE v2v_clips SET organization_json=COALESCE((SELECT organization_json FROM accepted_works WHERE id=v2v_clips.work_id),'{}');
