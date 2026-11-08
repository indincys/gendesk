CREATE TABLE live_photo_jobs (
    id INTEGER PRIMARY KEY,
    image_path TEXT NOT NULL UNIQUE,
    output_dir TEXT NOT NULL,
    error TEXT
);
