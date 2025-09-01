Data model and persistence
- Streams: parse quality/label/resolution; store bitrate/resolution; dedupe by (episode_id, url, quality).
- Subtitles: extend AssetKind and schema for subtitles (lang, format, url).
- Normalize languages and seasons/volumes (consistent codes/format).
- Add created_by_source on mapping tables to aid debugging.

Testing and tooling
- Integration tests: spin up a temp SQLite, run migrations, test get-or-create paths and uniqueness.
- Golden tests for mappings and serialization.
- Lint/fix warnings; gate CI on cargo fmt/clippy/tests.

Extensibility and daemon
- Library API: expose a clean async API for embedding (avoid internal runtime for library builds).
- Daemon mode (preview): start an HTTP/gRPC server with a minimal read-only API (search/list/streams).
