# Membrie architecture

## Invariants

1. Captured data remains on the user's computer.
2. SQLite is the canonical source of truth.
3. Original captured evidence is retained separately from replaceable derived data.
4. Search remains useful when Ollama is stopped.
5. Every Brie answer must cite exact supporting Remembries.
6. Vector indexes and model-produced artifacts must be safe to rebuild.

## Process boundary

`membried` is the only process that opens the canonical database. Clients use a
user-private Unix-domain socket and a length-limited, one-request-per-connection JSON
protocol. There is deliberately no TCP listener in the MVP.

The desktop app is a client. Capture adapters will also be clients, which prevents
Wayland- or application-specific capture code from gaining direct database access.

Quick Brie is a second, compact GTK application window in the same single-instance desktop client.
The GNOME bridge owns only its global shortcut and launches the app through GNOME's normal
activation path; it does not render assistant UI inside Shell. Before presenting the compact
window, the client may read the previously focused application's identity and title from the same
local bridge and append that limited metadata to the user's retrieval question. No live window
contents are read by Quick Brie. The compact client still reaches Brie exclusively through the
daemon's private Unix socket and loopback-only Ollama pipeline.

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
The capture helper also sends a small content-free heartbeat to the daemon. This health
timestamp exists only in daemon memory, avoiding needless database writes, and lets the
UI distinguish an installed bridge from a capture process that has stopped responding.
Application exclusions become fully effective for adapters that provide trusted
source-application context; arbitrary clipboard text still cannot reliably identify its
originating application under Wayland.

Activity context is independently disabled by default. When enabled, the same GNOME
bridge exposes the focused application, focused window title, lock state, and Mutter's
idle time only over the local session bus. The capture helper samples that state and the
daemon applies pause, application, window, content, private-window, password-manager,
and secret policies before persistence. Consecutive identical states update only the
session's last-active time; application or title changes create deduplicated observations.
An idle, lock, pause, exclusion, bridge loss, restart, manual finish, or disabled source
closes the session. The daemon then atomically materializes one time-bounded Activity
Remembrie and its enrichment job, keeping the Timeline and Brie's citations at the
meaningful-session level rather than exposing low-level input events. No key presses,
or pointer events are captured.

Screen Memory is a separate opt-in layered on activity context. The capture helper asks
the GNOME bridge for a one-shot PNG of only the focused window, never a continuous video.
It reduces the image to a small luminance fingerprint and discards frames that are not
materially different from the last useful sample. A changed PNG is accepted only while
the activity session remains open and the same pause, exclusion, private-window,
password-manager, title, and secret policies permit it. The daemon reads images only
from Membrie's private runtime spool, validates their type and size, and deletes each PNG
before asking the selected local vision model for a description. Ollama remains fixed to
`127.0.0.1`; the bridge itself has no network code.

Only the locally generated description, useful visible text, confidence, model provenance,
and active-window metadata enter SQLite. Model output is scanned again for recognizable
secrets. Activity Remembries label this material as machine-described, fallible supporting
context, and Brie is instructed not to treat a compose window or open form as proof that an
action was completed. Screen Memory is off by default and automatically turns off when
activity context is disabled. A deliberate capture control gives the user five seconds to
switch to the intended window. No screenshot pixels are retained in this first mode.

Each accepted screen sample gets a durable `processing` observation before local vision
begins. If its activity session closes while Ollama is still working, session enrichment is
held until that observation completes or fails. Successful late context is appended to the
canonical Remembrie and its replaceable search artifacts are rebuilt.

Semantic Context is another separate opt-in layered on Activity Context. Enabling it requires
explicit consent because GNOME's accessibility setting makes application-provided UI structure
available to Membrie and other local accessibility tools. The capture helper checks that setting
before every probe, reads the focused application identity and window title from the trusted GNOME
bridge, then scores only matching AT-SPI windows. Accessibility active/focused state is supporting
evidence rather than the selector because applications can retain stale state after losing focus.
An absent or tied match fails closed instead of inspecting a substitute window.

The selected probe has a 20-second deadline, visits at most 500 nodes to a maximum depth of 14,
collects only visible and showing nodes, and skips password-text nodes. It reads Accessible names,
descriptions, roles, states, and supported-interface metadata; it never constructs or invokes
Action or EditableText interfaces. The daemon applies the same pause, source, exclusion, private-
window, password-manager, secret, timestamp, and duplicate checks before a semantic observation
can enter SQLite. Rejected content produces only a content-free ledger decision.

Semantic and Screen Memory cooperate rather than blindly duplicating work. Rich semantic context
suppresses Screen Memory until the next semantic interval. Partial context is stored but permits
a visual fallback on the next desktop sample. Applications that do not expose usable AT-SPI data
are remembered only in capture-helper memory for 15 minutes, during which Screen Memory may fill
the gap. A policy-blocked semantic candidate never triggers a visual workaround. Semantic labels
are explicitly described in the finished Activity Remembrie as evidence of what was visible, not
proof that an action was completed.

The five-second compatibility test remains available as a store-nothing diagnostic. Its result
stays in app memory and is never sent to the daemon. Neither automatic Semantic Context nor the
compatibility test invokes Ollama or any network service.

Calendar access is an independent opt-in adapter. A small read-only helper uses Ubuntu's installed
Evolution Data Server introspection runtime to enumerate enabled calendar sources and expand event
instances across a bounded one-year history and one-year look-ahead. The helper has no create,
modify, delete, or refresh operation and never receives account credentials. Calendar providers
already configured in GNOME remain owned by the desktop and may perform their normal synchronization
independently; Membrie reads only the local EDS view.

The daemon validates and bounds every helper field, applies application/content exclusions and
high-confidence secret detection before persistence, and reconciles changed or removed instances
transactionally. Successful sources can be reconciled even when another source fails; a failed
source's existing records are preserved. Calendar instances become explicitly typed `calendar`
Remembries so FTS, local embeddings, Timeline evidence, and Brie citations use the same canonical
path as other evidence. Disabling access stops future reads without silently deleting previously
remembered events.

## Storage

SQLite owns identity, timestamps, metadata, captured text, processing state, and
relationships. FTS5 maintains a lexical index. Binary attachments will live in a
content-addressed directory and be referenced by hash.

The daemon creates backups with SQLite's online backup API, verifies them with
`integrity_check`, syncs the completed file before publishing it, and retains a rolling
set of 14 snapshots. A partial or failed snapshot is never presented as a backup.

Embeddings are modeled as derived artifacts with model provenance. Vector retrieval
is isolated behind the search service so its implementation can change without
changing the canonical Remembrie model. Summaries, chunks, and vectors are replaceable;
original evidence remains canonical.

## Local intelligence boundary

The daemon connects to Ollama only at the fixed loopback address
`127.0.0.1:11434`. It does not honor a remote host environment variable, and it rejects
cloud-backed model names. The default generation model is `gemma4:12b` with an 8192-token
working context; `embeddinggemma` produces the semantic vectors.
Document chunks and queries use EmbeddingGemma's asymmetric retrieval prompts: indexed
chunks are labeled as documents, while Search and Brie questions use their respective
query tasks. This keeps one local document index useful for both recall paths.

Every new Remembrie gets a durable `enrich` job in the same transaction as its canonical
record. One background worker claims one job at a time, releases the database lock while
Ollama works, and atomically installs the derived summary, chunks, and embeddings. A
daemon interruption returns running work to the pending queue on restart. Repeated
failures use bounded backoff and remain explicitly retryable in the UI.

Captured Remembries are untrusted evidence. Brie is instructed never to follow commands
inside retrieved content, and its structured response must identify supporting source
numbers. Membrie validates those numbers and withholds unsupported answers rather than
displaying uncited model output.

## Retrieval

The hybrid retrieval pipeline is:

1. FTS5/BM25 candidates
2. local `embeddinggemma` candidates
3. cosine-similarity relevance thresholds
4. grouping the strongest chunk back to each Remembrie
5. reciprocal-rank fusion
6. citation-preserving context assembly for Brie

Brie receives only the highest-ranked evidence within its bounded local context. Search
falls back to FTS5 when Ollama is unavailable, so exact recall never depends on a model.

## Timeline projection

The Timeline is a read-only projection of canonical Remembries and activity-session ledgers.
Its day, week, and month maps measure remembered active time only. Color identifies the dominant
application in each period, while intensity reflects duration; the design deliberately avoids
streaks and productivity scores. A daily ribbon derives each application's duration from
consecutive focus observations while preserving idle gaps. Application identities map
deterministically onto a small accessible color palette, with names and durations always retained
so meaning never depends on color alone.

Day entries remain grouped at the meaningful Remembrie level. Exact captured evidence is loaded
on demand from the daemon, keeping the normal Timeline response compact. Lightweight repeated-
context observations are deterministic and evidence-linked; they are phrased as patterns worth
noticing rather than claims about intent or task completion. Relationship data remains available
to Search and Brie without requiring a separate constellation interface.

Brie's citations use the same Remembrie identity as the Timeline. Opening a citation selects the
source's local calendar day, highlights and scrolls to that entry, and loads its exact evidence
from the daemon rather than relying on the answer's abbreviated excerpt.
