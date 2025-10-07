-- Initial schema for Touring aggregator
-- SQLite dialect

-- Sources (plugins/providers)
CREATE TABLE IF NOT EXISTS sources (
  id            TEXT PRIMARY KEY,          -- e.g., "mangadex_plugin"
  version       TEXT NOT NULL,
  created_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Series (generic: manga or anime)
CREATE TABLE IF NOT EXISTS series (
  id            TEXT PRIMARY KEY,          -- canonical local id (uuid/ulid)
  kind          TEXT NOT NULL CHECK (kind IN ('manga','anime')),
  title         TEXT NOT NULL,
  alt_titles    TEXT,                      -- JSON array of strings
  description   TEXT,
  cover_url     TEXT,
  tags          TEXT,                      -- JSON array of strings
  status        TEXT,                      -- e.g., ongoing/completed
  created_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Mapping Series <-> Source-specific IDs
CREATE TABLE IF NOT EXISTS series_sources (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  series_id      TEXT NOT NULL,
  source_id      TEXT NOT NULL,
  external_id    TEXT NOT NULL,
  last_synced_at DATETIME,
  UNIQUE(series_id, source_id, external_id),
  FOREIGN KEY(series_id) REFERENCES series(id) ON DELETE CASCADE,
  FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
);

-- Chapters (for manga)
CREATE TABLE IF NOT EXISTS chapters (
  id            TEXT PRIMARY KEY,
  series_id     TEXT NOT NULL,
  source_id     TEXT NOT NULL,
  external_id   TEXT NOT NULL,
  number_text   TEXT,                      -- raw chapter number representation
  number_num    REAL,                      -- parsed float for sorting if available
  title         TEXT,
  lang          TEXT,
  volume        TEXT,
  upload_group  TEXT,
  published_at  DATETIME,
  created_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE(series_id, source_id, external_id),
  FOREIGN KEY(series_id) REFERENCES series(id) ON DELETE CASCADE,
  FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
);

-- Chapter images (ordered pages)
CREATE TABLE IF NOT EXISTS chapter_images (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  chapter_id TEXT NOT NULL,
  idx        INTEGER NOT NULL,             -- page index starting at 1
  url        TEXT NOT NULL,
  mime       TEXT,
  width      INTEGER,
  height     INTEGER,
  UNIQUE(chapter_id, idx),
  FOREIGN KEY(chapter_id) REFERENCES chapters(id) ON DELETE CASCADE
);

-- Episodes (for anime) - mirrors chapters
CREATE TABLE IF NOT EXISTS episodes (
  id            TEXT PRIMARY KEY,
  series_id     TEXT NOT NULL,
  source_id     TEXT NOT NULL,
  external_id   TEXT NOT NULL,
  number_text   TEXT,
  number_num    REAL,
  title         TEXT,
  lang          TEXT,
  season        TEXT,
  upload_group  TEXT,
  published_at  DATETIME,
  created_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE(series_id, source_id, external_id),
  FOREIGN KEY(series_id) REFERENCES series(id) ON DELETE CASCADE,
  FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
);

-- Streams (for anime episodes) - mirrors chapter_images
CREATE TABLE IF NOT EXISTS streams (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  episode_id  TEXT NOT NULL,
  url         TEXT NOT NULL,
  quality     TEXT,
  mime        TEXT,
  FOREIGN KEY(episode_id) REFERENCES episodes(id) ON DELETE CASCADE
);

-- Search cache (persisted subset)
CREATE TABLE IF NOT EXISTS search_cache (
  key         TEXT PRIMARY KEY,            -- source|kind|query normalized
  payload     TEXT NOT NULL,               -- JSON (array of series summaries)
  created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  expires_at  DATETIME NOT NULL
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_series_kind_title ON series(kind, title);
CREATE INDEX IF NOT EXISTS idx_series_sources_external ON series_sources(external_id, source_id);
CREATE INDEX IF NOT EXISTS idx_chapters_series_number ON chapters(series_id, number_num);
CREATE INDEX IF NOT EXISTS idx_chapters_source_external ON chapters(source_id, external_id);
CREATE INDEX IF NOT EXISTS idx_episodes_series_number ON episodes(series_id, number_num);
CREATE INDEX IF NOT EXISTS idx_search_cache_exp ON search_cache(expires_at);
