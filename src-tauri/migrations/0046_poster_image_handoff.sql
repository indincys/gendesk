-- 审稿与发布交接分开：只有用户在审稿后的弹窗确认，才允许进入 Poster。
-- 不设作品外键：批次退休、作品移入废纸篓，都不能使已确认的交接失忆。
CREATE TABLE poster_batches (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('preparing','awaiting','skipped','queued','done','failed')),
    last_error TEXT NOT NULL DEFAULT '',
    request_json TEXT,
    receipt_json TEXT,
    library_root TEXT NOT NULL DEFAULT ''
);
CREATE TABLE poster_images (
    batch_id TEXT NOT NULL REFERENCES poster_batches(id),
    work_id INTEGER NOT NULL,
    uid TEXT NOT NULL,
    item_name TEXT NOT NULL,
    sku TEXT NOT NULL,
    style TEXT NOT NULL,
    image_path TEXT NOT NULL,
    product_id TEXT,
    product_name TEXT,
    PRIMARY KEY(batch_id,work_id)
);
CREATE INDEX idx_poster_batches_state ON poster_batches(state,created_at);
