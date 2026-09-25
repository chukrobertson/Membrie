# Membrie architecture

## Invariants

1. Captured data remains on the user's computer.
2. SQLite is the canonical source of truth.
3. Original captured evidence is retained separately from replaceable derived data.
4. Search remains useful when Ollama is stopped.
5. Every future Brie answer must cite exact supporting Remembries.
6. Vector indexes and model-produced artifacts must be safe to rebuild.

## Process boundary

`membried` is the only process that opens the canonical database. Clients use a
user-private Unix-domain socket and a length-limited, one-request-per-connection JSON
protocol. There is deliberately no TCP listener in the MVP.

The desktop app is a client. Capture adapters will also be clients, which prevents
Wayland- or application-specific capture code from gaining direct database access.

## Safe capture boundary

Automatic candidates are evaluated in daemon memory before persistence. The order is:

1. pause state
2. size and empty-content limits
3. application, window, domain, and content exclusions
4. high-confidence local secret detection
5. recent duplicate detection
6. persistence and indexing

Skipped candidates produce only a content-free ledger event with a category and
reason. The original candidate is not inserted into SQLite, FTS, logs, or processing
jobs. Manual notes remain explicit user actions and bypass automatic-capture policy.

Clipboard capture is disabled by default because Wayland does not reliably expose the
application that owns clipboard content to an unfocused GTK process. A deliberately
small GNOME Shell extension observes Mutter's clipboard-owner changes and exports only
the MIME types at first. The Rust capture agent requests a guarded read only when
clipboard capture is enabled and no password-manager hint is present. The extension then
retrieves text through GNOME Shell's text-clipboard API while suppressing the temporary
owner-change event caused by that read, and makes the result available as a one-shot
local D-Bus value. The agent forwards it to the daemon's policy boundary over the private
Unix socket. There is no network transport in this path.

The desktop UI checks that the bridge actually owns its D-Bus name. It must never show
clipboard capture as available merely because the database preference is enabled.
Application exclusions become fully effective for adapters that provide trusted
source-application context; arbitrary clipboard text still cannot reliably identify its
originating application under Wayland.

## Storage

SQLite owns identity, timestamps, metadata, captured text, processing state, and
relationships. FTS5 maintains a lexical index. Binary attachments will live in a
content-addressed directory and be referenced by hash.

Embeddings are modeled as derived artifacts with model provenance. Vector retrieval
will be added behind the search service so its implementation can change without
changing the canonical Remembrie model.

## Retrieval

The target hybrid retrieval pipeline is:

1. FTS5/BM25 candidates
2. local embedding candidates
3. deterministic metadata and time filters
4. reciprocal-rank fusion
5. optional local reranking
6. grouping from chunks back to Remembries
7. citation-preserving context assembly for Brie

The first vertical slice implements step 1 and keeps the response shape compatible
with later fused results.
