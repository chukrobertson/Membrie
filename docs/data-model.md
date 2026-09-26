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
- `semantic_observations`: bounded, policy-approved visible labels and text supplied by
  application accessibility interfaces, tied to activity sessions and quality-labeled
- `screen_observations`: explicitly labeled local vision descriptions tied to activity sessions;
  processing state survives session boundaries and temporary source images are not retained

Semantic observations and completed screen descriptions become labeled sections of the same
Activity Remembrie when its session closes. They are supporting context rather than independent
claims that an action occurred. Disabling Activity Context also disables both dependent sources.

## Derived records

- `remembrie_fts`: replaceable full-text projection
- `remembrie_chunks`: citation-addressable excerpts for retrieval
- `embeddings`: vectors with model and dimensionality provenance
- `derived_artifacts`: summaries, future dedicated OCR, entities, and other model output
- `processing_jobs`: durable background work and retry state
- `intelligence_settings`: selected local Ollama models and bounded working context

Deleting or regenerating derived records must never destroy captured evidence.
