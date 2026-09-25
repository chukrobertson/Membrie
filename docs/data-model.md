# Remembrie data model

The user-visible unit is a **Remembrie**. A Remembrie represents a meaningful moment,
not an individual key press or low-level operating-system event.

## Canonical records

- `remembries`: identity, time range, source context, importance, and privacy state
- `remembrie_contents`: original or user-authored text and future blob references
- `remembrie_links`: explicit and inferred relationships between Remembries
- `capture_rules`: exclusions and capture policy
- `capture_state`: global pause state and opt-in source settings
- `capture_events`: content-free decisions for stored or skipped automatic events
- `activity_sessions`: bounded active-use periods and their closing reason
- `activity_observations`: deduplicated focused application and window-title changes

## Derived records

- `remembrie_fts`: replaceable full-text projection
- `remembrie_chunks`: citation-addressable excerpts for retrieval
- `embeddings`: vectors with model and dimensionality provenance
- `derived_artifacts`: summaries, OCR, entities, and other model output
- `processing_jobs`: durable background work and retry state
- `intelligence_settings`: selected local Ollama models and bounded working context

Deleting or regenerating derived records must never destroy captured evidence.
