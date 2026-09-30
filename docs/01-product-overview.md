# 1. Product Overview

## What Recall is

Recall is a **local-first desktop application for managing a personal
digital archive**. It takes a user's existing photo / video / document
collection — from phones, cameras, Google Takeout exports, manual folders
— and turns it into a single, content-addressed, verifiable archive on
the user's own disk, with structured metadata preserved and a UI for
browsing and searching it.

Recall is **not** a cloud product, **not** a photo editor, and **not** a
social platform. It is durable storage plus a personal search index.

## Who it's for

- People who take a lot of photos and videos and want one canonical
  archive that survives phone upgrades, broken SD cards, and revoked
  cloud accounts.
- People who actively want to **own** their files and metadata locally
  rather than rely on Google / Apple / Dropbox.
- People who care about **data integrity** more than visual polish:
  they'll tolerate a CLI in exchange for "I can prove every byte is
  there and unchanged."

## Design values

These are the values every other decision in this project defers to:

1. **Preservation over polish.** Never re-encode, never re-compress,
   never strip metadata to "clean things up." If a user wants to edit,
   they edit a copy.
2. **Local first.** No required network calls. No telemetry. The app
   must work on a plane.
3. **Integrity is provable, not just claimed.** SHA-256 hashing on
   ingest, periodic verification, atomic writes, content-addressed
   storage so dedup is automatic.
4. **Crash-safe by default.** Mid-write failures leave at most a
   `*.partial` file behind; mid-import failures leave a finished row in
   `imports` with whatever was recorded.
5. **Idempotent.** Re-running any operation on the same source must be
   a no-op for already-archived content.
6. **Boring technology where it works.** SQLite, the filesystem, the
   operating system's `fsync`. No exotic dependencies unless they earn
   their place.

## What this means for trade-offs

- We chose **content-addressable storage** (objects keyed by sha256)
  even though it requires writing extra plumbing for the deduplication
  win, because "I can store the same photo twice from my phone and
  Google Takeout without it taking 2x space" is exactly what a personal
  archive needs.
- We chose **sidecar JSON preservation** (raw Google JSON retained
  verbatim) instead of "just parse out the interesting fields," because
  future tools may need fields we didn't parse today.
- We chose **no re-encoding** even when the user already has a 50 MB
  HEIC; storage is cheap, lost detail is forever.
- We chose to **defer the GUI** (Tauri) until the engine and CLI are
  stable, because the engine's API surface is the only thing a UI
  can't fix later.
