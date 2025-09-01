-- Ensure streams are unique per episode by URL
CREATE UNIQUE INDEX IF NOT EXISTS idx_streams_episode_url ON streams(episode_id, url);
