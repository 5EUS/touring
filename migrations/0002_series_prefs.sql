-- Series preferences/settings
CREATE TABLE IF NOT EXISTS series_prefs (
  series_id     TEXT PRIMARY KEY,
  download_path TEXT,
  created_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY(series_id) REFERENCES series(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_series_prefs_path ON series_prefs(download_path);
