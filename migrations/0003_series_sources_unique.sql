-- Add unique index to speed up canonical series lookup by provider mapping
CREATE UNIQUE INDEX IF NOT EXISTS idx_series_sources_source_external
ON series_sources(source_id, external_id);
