-- Track per-chapter reading progress (page position cache)
CREATE TABLE IF NOT EXISTS chapter_progress (
  chapter_id   TEXT PRIMARY KEY,
  series_id    TEXT NOT NULL,
  page_index   INTEGER NOT NULL,
  total_pages  INTEGER,
  updated_at   INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(chapter_id) REFERENCES chapters(id) ON DELETE CASCADE,
  FOREIGN KEY(series_id) REFERENCES series(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_chapter_progress_series ON chapter_progress(series_id);
