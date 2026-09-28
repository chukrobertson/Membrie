# Remembrie data model

The user-visible unit is a **Remembrie**. A Remembrie represents a meaningful moment,
not an individual key press or low-level operating-system event.

## Canonical records

- `remembries`: identity, time range, source context, importance, and privacy state
- `remembrie_contents`: original or user-authored text, attachment references, labeled local analysis,
  and an optional user correction stored separately from the machine interpretation
- `attachment_blobs`: unique SHA-256 identities, byte sizes, and detected media types for retained files
- `remembrie_links`: explicit and inferred relationships between Remembries
- `capture_rules`: exclusions and capture policy
- `capture_state`: global pause state and opt-in source settings
- `capture_events`: content-free decisions for stored or skipped automatic events
- `activity_sessions`: bounded active-use periods and their closing reason
- `activity_observations`: deduplicated focused application and window-title changes
- `semantic_observations`: bounded, policy-approved visible labels and text supplied by
  application accessibility interfaces, tied to activity sessions and quality-labeled
- LibreOffice context remains in `semantic_observations`, with explicit suite, component, and
  focused-document labels plus a bounded read-only excerpt in `text_content`
- `screen_observations`: explicitly labeled local vision descriptions tied to activity sessions;
  processing state survives session boundaries and temporary source images are not retained
- `mobile_usage_events`: coarse opening counts and bounded visible-time pulses, stored locally without
  page-level behavior or device fingerprinting

Semantic observations and completed screen descriptions become labeled sections of the same
Activity Remembrie when its session closes. They are supporting context rather than independent
claims that an action occurred. Disabling Activity Context also disables both dependent sources.

The visual Timeline does not create another canonical table. History cells, per-application
duration ribbons, and repeated-context observations are derived on demand from Remembries,
activity sessions, and their observations. This keeps colors and presentation replaceable while
the original evidence remains authoritative.

## Derived records

- `remembrie_fts`: replaceable full-text projection
- `remembrie_chunks`: citation-addressable excerpts for retrieval
- `embeddings`: vectors with model and dimensionality provenance
- `derived_artifacts`: summaries, future dedicated OCR, entities, and other model output
- `processing_jobs`: durable background work and retry state
- `intelligence_settings`: selected local Ollama models and bounded working context

Deleting or regenerating derived records must never destroy captured evidence.

Attachment bytes are canonical evidence but deliberately live outside SQLite under
`blobs/sha256/<prefix>/<hash>`. The database owns their references and lifecycle. Identical files
deduplicate by hash; a single Remembrie can reference a reviewed set of attachments; complete backups
contain both the verified database and every referenced blob.
Image descriptions, extracted visible text, and timestamped voice-note transcripts are derived,
fallible, model-labeled material and can be retried or regenerated without changing the retained
original.
When a user corrects a voice transcript, Membrie retains both the machine text and the separate
user-authored correction; Brie is instructed to state that distinction if they conflict.
