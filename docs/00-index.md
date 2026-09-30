# Recall — Documentation Index

This folder is the authoritative spec for the Recall project.

## Reading order

1. **`01-product-overview.md`** — what Recall is, who it's for, and the
   design values that drive every trade-off.
2. **`02-system-requirements-specification.md`** (SRS) — features,
   functional and non-functional requirements, scope boundaries.
3. **`03-system-design.md`** — architecture, crate layout, data model,
   storage layout, and the contracts between layers.
4. **`04-current-status.md`** — what's actually built today, with links to
   the commands and tests that prove it.
5. **`05-roadmap.md`** — what remains, in the serial order it should be
   tackled, with rationale.

## Conventions in these docs

- Requirements are tagged `FR-…` (functional) and `NFR-…` (non-functional).
- Code references use crate paths: `recall-importer::import_takeout_zip`.
- Schema references use `table.column`.
- Anything in **`05-roadmap.md`** is **not yet built**. Anything in
  **`04-current-status.md`** is built and tested.
