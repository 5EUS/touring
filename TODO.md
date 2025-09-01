Correctness and stability
- Capabilities refresh: add a method to refresh/get capabilities at runtime and a CLI command to print them.
- Timeouts and retries: add per-plugin timeouts, backoff, and error classification; don’t block the whole search.
- Concurrency: run plugin calls in parallel with a small worker pool; guard with rate limiting.

Data model and persistence
- Streams: parse quality/label/resolution; store bitrate/resolution; dedupe by (episode_id, url, quality).
- Subtitles: extend AssetKind and schema for subtitles (lang, format, url).
- Normalize languages and seasons/volumes (consistent codes/format).
- Add created_by_source on mapping tables to aid debugging.

Caching and performance
- Search cache: cache search results per source+kind+query with TTL; invalidate on explicit refresh.
- Chapter image cache: add ETag/If-Modified-Since support when providers expose it; configurable TTLs.

CLI and UX
- Show source in outputs (e.g., “Anime [mangadex]: Title”).
- Add --plugins-dir and --json output for machine readability.
- Add commands: capabilities, refresh-cache, vacuum-db.

Testing and tooling
- Integration tests: spin up a temp SQLite, run migrations, test get-or-create paths and uniqueness.
- Golden tests for mappings and serialization.
- Lint/fix warnings; gate CI on cargo fmt/clippy/tests.

Extensibility and daemon
- Library API: expose a clean async API for embedding (avoid internal runtime for library builds).
- Daemon mode (preview): start an HTTP/gRPC server with a minimal read-only API (search/list/streams).

Security and config
- WASI HTTP allowlist of hosts per plugin; configurable user-agent and proxy.
- Plugin config file format (toml/json) to declare capabilities, rate limits, and allowed hosts.