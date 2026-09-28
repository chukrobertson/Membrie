# Membrie development roadmap

This roadmap records Membrie's current direction. It is a guide rather than a
release promise: privacy, evidence quality, and practical use remain more important
than checking off features.

## Product boundary

The Ubuntu PC is Membrie's private home. The database, retained evidence, search
indexes, models, and model inference remain there. Companion devices are trusted
windows into that home; they do not become replicas or second database owners.

Users provide a private network path to the loopback-only Companion gateway.
Tailscale remains the polished, documented default because it already works well,
but Membrie's internal trust boundary will use an exact approved HTTPS origin rather
than permanently encoding one network provider. This leaves a narrow, auditable seam
for Headscale or another private reverse proxy without turning Membrie into a public
internet service or a broad networking support matrix. Public exposure and Tailscale
Funnel are not goals.

## 1. Companion trust and dependable capture

- Give every Companion installation its own named identity and unique token, stored
  only as a hash on the PC.
- Show last use and coarse local usage per device, with rename and immediate revoke
  controls. Do not collect fingerprints, IP histories, content telemetry, or remote
  analytics.
- Add QR-based device setup while preserving the existing token as a deliberate
  fallback.
- Migrate an actively used legacy pairing in place, without forcing Chuk or existing
  users to register that installed Companion again. Unused browser pairings can stay
  visibly labeled as legacy until upgraded or revoked.
- Make iPhone onboarding explicit: install the home-screen WebUI first, then pair it,
  because the browser tab and installed WebUI have separate storage.
- Add an offline outbox, visible pending state, retry, and an explicit PC receipt so a
  phone note or attachment never merely appears to have arrived.
- Keep coarse usage measurement limited to useful local facts such as opens, bounded
  visible time, and capture counts.

## 2. Ownership, recovery, and storage

- Keep the Timeline's local-storage meter current and honest. It covers the database,
  local models, retained attachments, and verified backups, counting hard-linked
  backup data only once.
- Add retention controls with clear impact previews before anything is removed.
- Add guided backup restore and integrity verification in the app.
- Add human-usable export for Remembries and retained original evidence.
- Support a user-selected backup destination while keeping the canonical database on
  the local PC.

## 3. Richer local documents

- Understand PDFs with page-aware text, local OCR where needed, and citations back to
  the retained original.
- Extend document understanding across ODF and common OOXML files without requiring a
  cloud office account.
- Preserve the distinction between original evidence and replaceable machine-derived
  descriptions, summaries, and embeddings.

## 4. Cross-source corroboration

- Relate activity, semantic context, Screen Memory, notifications, calendar events,
  notes, attachments, and Thunderbird evidence when their timing and subjects support
  the relationship.
- Let Brie explain which sources agree, which conflict, and what remains uncertain.
- Surface useful patterns without attention scoring, surveillance language, or claims
  stronger than the evidence.

## 5. Meeting Memory

- Explore an explicitly enabled PipeWire capture path for meetings and calls.
- Produce local, time-aligned transcripts and summaries while preserving truthful
  speaker and delivery uncertainty.
- Make recording state unmistakable and keep pause, exclusion, retention, and deletion
  controls close at hand.

## 6. Public alpha readiness

- Finish first-run guidance, permissions explanations, health checks, and recovery
  paths for a clean Ubuntu installation.
- Test upgrades and migrations against real retained data.
- Publish a concise privacy model, threat boundary, contribution guide, and known
  limitations.
- Keep the project free software under AGPL-3.0-or-later and usable without a paid
  account or subscription.

## Continuing non-goals

- No hosted Membrie cloud, remote analytics, advertising, or third-party LLM API.
- No keyboard or pointer logging.
- No continuous screen video archive.
- No productivity score or automatic judgment of the user.
- No claim that a visible draft, notification, calendar event, or compose window proves
  an action happened.
