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
protocol. The database daemon never exposes TCP.

The desktop app is a client. Capture adapters will also be clients, which prevents
Wayland- or application-specific capture code from gaining direct database access.

The optional Mobile Companion is a narrow gateway rather than a second database owner. It talks to
the daemon through the same private Unix socket and initially accepts HTTP only on
`127.0.0.1:47381`. Its static interface has no external scripts, fonts, trackers, or service
workers. Every API request requires a 256-bit token stored in Membrie's private data directory;
tailnet membership does not replace that application-level pairing.

The gateway checks GNOME's lock state before every reading operation and fails closed when that
state cannot be read. A paired device may still submit a deliberate note or attachment set while the PC is locked,
but Recall, Brie, and clipboard reads or writes return a locked response unless the user separately
enables locked-session access. Response previews are bounded, likely secrets are blocked from
clipboard transfer, request bodies have hard limits, and private API responses are never cached.
The gateway permits its own microphone only so a user gesture can start a Companion voice note;
camera and unrelated browser permissions remain disabled.

Normal device setup uses a private invitation file created by the desktop app. It contains a random
8-digit code and a five-minute expiry, is mode `0600`, and is protected by a process-wide pairing
lock plus a delay on every failed attempt. The code is removed after one successful exchange. A QR
encodes the tailnet URL and temporary code; the WebUI removes that fragment from browser history
before exchanging it for the 256-bit reusable token. The full token remains available only as a
deliberate fallback. A browser tab and an installed home-screen WebUI are separate browser storage
containers and therefore pair independently.

The service accepts only a local Host and Origin until an exact `.ts.net` hostname has been written
to its private configuration. It then accepts that hostname only over an HTTPS Origin. Tailscale
Serve terminates tailnet HTTPS and forwards to the same loopback listener; the companion still
rejects non-loopback peers, and the database daemon remains behind its Unix-socket boundary.
Tailscale identity and ACLs are an outer boundary, while Membrie's independent pairing token and
locked-session policy remain the application boundary. Tailscale Funnel is never used.

Notes submitted through the companion remain evidence kind `note`, because they are deliberate
user-authored recollections rather than observations. Their source is recorded as `Membrie Companion`
and their window provenance as `Mobile WebUI`. Brie uses that exact provenance when a question names
mobile, phone, iPhone, Companion, or WebUI notes, and combines it with explicit recency language. Older
notes created before provenance existed are never retroactively relabeled by guesswork.

Companion capture is deliberately user-initiated. A paste button uses only clipboard representations
that the browser exposes after that gesture; a dedicated paste surface supports iOS's system Paste
action when programmatic clipboard reading is unavailable. Text and links are merged into the note,
while exposed binary items and picker selections become a reviewed attachment tray. One submission
creates one Remembrie with at most eight attachments and 100 MiB of combined retained data. Partial
uploads can be explicitly discarded, and abandoned private staging files expire locally after one day.

The companion records only coarse local usage events: an opening count and bounded active-time
pulses while the page is visible. It records no navigation path, device fingerprint, analytics
identifier, or remote telemetry. Rolling day, week, and month totals stay in the canonical database.

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

The same bridge supports an explicit, on-demand image ferry for PNG, JPEG, and WebP clipboard
content. It reads bytes only after a paired Companion user presses **Fetch**, caps the result at
32 MiB, and returns it over the local session bus. The gateway verifies both the declared media type
and file signature before returning the image through private Tailscale HTTPS. It is not automatic
clipboard capture: the temporary phone preview is stored only after a separate **Remember** action.

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

LibreOffice is a first-class specialization inside that same boundary rather than a separate file
reader. The probe recognizes Writer, Calc, Impress, Draw, Base, and Math from the focused AT-SPI
window, derives the active document identity from its title, and may call only read operations on
the Text interface for visible document, paragraph, text, heading, comment, or cell nodes. For
virtualized sheets and slides, the read-only side of Selection may identify at most 16 selected
descendants rather than expanding a huge document tree. Text reads are capped at 24 nodes, 2,000
characters per node, and 6,000 characters per observation. Long nodes use a bounded range around
the reported caret; Membrie does not enumerate files, open inactive documents, request unsaved
document contents through UNO, or invoke EditableText, Action, or any selection-changing operation.
A useful document excerpt marks the semantic sample rich enough to avoid redundant local vision
work. Downstream exclusions and secret detection remain unchanged.

Thunderbird uses the same focused-window boundary and remains an accessibility integration—not a
Gmail login or mail-protocol client. Thunderbird owns its configured accounts and performs its normal
network synchronization; Membrie receives no credentials and reads no Thunderbird profile database.
The probe recognizes the Thunderbird snap and desktop identities, records bounded accessible names
and message text, and follows at most 16 selected descendants in virtualized message lists. Compose,
Drafts, Outbox, Inbox, and Sent contexts receive explicit evidence labels: a compose or draft window
does not prove sending; Outbox may mean queued; a message visibly listed in Sent supports that
Thunderbird submitted it or stored a sent copy, but does not prove delivery or reading. Brie's local
system prompt repeats these boundaries before answering from the resulting Remembries.

The GNOME bridge also watches the Shell's already-local notification objects and filters them to a
trusted Thunderbird application identity before emitting anything to the capture helper. A newly
added notification, or a changed Thunderbird notification, is accepted only while the separate,
off-by-default Thunderbird-arrivals source is enabled and capture is not paused. Title and body are bounded, normalized,
and passed through the ordinary application/content exclusions, secret detection, and duplicate
filter before becoming a `mail_arrival` Remembrie. This is notification evidence, not direct mailbox
access: if Thunderbird notifications are disabled or GNOME never receives one, Membrie has no arrival
event. It never opens the message or treats arrival as evidence of reading or response.

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
relationships. FTS5 maintains a lexical index. Binary attachments live outside SQLite in a private
SHA-256 content-addressed directory and are referenced by hash. Repeated identical files share one
stored blob. A deliberate attachment retains the original bytes as canonical evidence while its
filename, media type, processing state, model, confidence, and generated interpretation remain
attached to the Remembrie in SQLite.

The image attachment analyzer accepts PNG, JPEG, GIF, and WebP images up to 32 MiB. It sends the
retained bytes only to the selected loopback Ollama vision model, stores the result as explicitly
unverified machine description, and queues ordinary summary and embedding work afterward. Secret-like
model output is withheld. A separate speech-first analyzer accepts common audio attachments up to 30
minutes. A fixed local FFmpeg command decodes them into a private temporary mono WAV, Ubuntu's
`whisper.cpp` transcribes them with a verified model under Membrie's data directory, and the temporary
audio is removed immediately afterward. The timestamped transcript is explicitly unverified,
secret-like text is withheld, and recordings with no clear speech are labeled accordingly. Other
formats and larger retained images remain available as originals but are labeled unsupported rather
than silently interpreted. Deleting the last Remembrie that references a blob deletes that blob as
well.

A transcript correction is stored in its own database field and incorporated into search with an
explicit user-authored label. It never overwrites the machine transcript or retained original.
Attachment retry clears only the replaceable machine interpretation and queues local processing
again; a user correction remains intact. Deletion prunes content-addressed bytes only after no
remaining Remembrie references them.

Unknown and not-yet-understood attachment formats are never executed or presented as analyzed.
Their filename, detected media type, size, and retained-original state remain visible. This lets
capture support arrive independently from later PDF, office-document, and video analyzers.

The daemon creates backups with SQLite's online backup API, verifies them with
`integrity_check`, copies or hard-links every referenced attachment, writes a manifest, syncs the
completed private directory before publishing it, and retains a rolling set of 14 backups. A partial
or failed backup is removed and never presented as complete. Legacy database-only snapshots remain
visible alongside the new complete backup directories.

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

Brie's retrieval context includes the canonical kind and local source for every record. Her system
boundary treats calendar records as plans, activity records as focus/time evidence, clipboard
records as copied text, manual notes as user-authored recollection, and visual descriptions as
fallible machine output. Claims of sending, attending, or completing something require explicit
support from observed evidence rather than inference from a schedule or open window.

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
on demand from the daemon, keeping the normal Timeline response compact. Patterns are derived on
demand rather than stored as canonical claims. The first deterministic detectors cover repeated
focus visits, resuming the same first/last context after a meaningful break, and clusters of local
Thunderbird arrival notifications. Every result carries its caution and exact supporting Remembrie
IDs; Timeline evidence buttons navigate to those records. No detector scores productivity, importance,
emotion, intent, or completion. When a question explicitly asks about patterns, Brie receives these
derived pointers plus the canonical supporting sources and must cite those sources. Relationship data
remains available without requiring a separate constellation interface.

The chronology can separate observed records, scheduled calendar records, and manually authored
notes, and it permits bounded navigation into the same one-year future window used by the calendar
adapter. Every exact-evidence view names the local source, evidence type, event time, storage time,
and an interpretation warning. A scheduled event is never presented as proof of attendance or
completion.

Brie's citations use the same Remembrie identity as the Timeline. Opening a citation selects the
source's local calendar day, highlights and scrolls to that entry, and loads its exact evidence
from the daemon rather than relying on the answer's abbreviated excerpt.
