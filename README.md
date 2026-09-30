# Recall

> A private, local-first photo and video library. Your files stay on your Mac —
> no cloud, no account, no upload.

Recall is a desktop app that imports photos and videos from a folder (or from
a Google Takeout export), deduplicates by content, extracts EXIF / GPS, and
lets you search, browse, and back up the result. Everything is stored in a
single folder you choose — the same folder you can copy to an external drive
or a NAS for safekeeping.

## Why

Apple Photos, Google Photos, and Immich all assume you trust someone with
your memories. Recall doesn't. It's a single Rust binary with a Tauri webview
UI, the same shape as a CLI tool you'd write for yourself.

## Features

- **Import a folder** — drops into a content-addressed store (SHA-256), so
  re-imports are deduped for free.
- **Import a Google Takeout** — extracts albums + sidecar metadata.
- **Search** — by date, kind, album, free text, or GPS proximity.
- **Verify** — re-hash every archived object to catch bit-rot.
- **Back up** — copy the archive to a second drive.
- **Export** — bundle the archive + DB into a single `.zip`.
- **Local-only.** No accounts, no telemetry by default. Anonymous crash
  reports are opt-in.

## Quick start

```sh
# Install Rust + the Tauri CLI once
cargo install tauri-cli --version "^2"

# Build a release binary
cargo tauri build --manifest-path crates/recall-app/Cargo.toml

# Open the .app
open target/release/bundle/macos/Recall.app
```

On first launch, the **Setup Wizard** walks you through picking an archive
location and (optionally) importing 20 sample photos so you can see the app
working before trusting it with real data.

## Headless mode

`recallctl` is the headless twin of the app — same engine, no GUI:

```sh
cargo run --release -p recallctl -- init
cargo run --release -p recallctl -- import folder --source ~/Photos
cargo run --release -p recallctl -- import takeout --source ~/takeout-extracted
cargo run --release -p recallctl -- verify
cargo run --release -p recallctl -- status
```

## Architecture

```
crates/
  recall-core         # shared types (MediaId, ArchivePath, Progress)
  recall-hash         # SHA-256 helpers
  recall-storage      # content-addressed object store
  recall-db           # SQLite catalog + query layer
  recall-metadata     # EXIF + sidecar parsers
  recall-importer     # folder + takeout walkers
  recall-verify       # integrity checker
  recall-backup       # copy + zip export
  recallctl           # headless CLI
  recall-app          # Tauri 2 desktop UI
```

The engine crates have zero knowledge of Tauri. `recall-app/src/commands.rs`
is the only file that knows about webviews.

## Building from source

See [`crates/recall-app/README.md`](crates/recall-app/README.md) for the
detailed build instructions, dependency list, and how to run with a custom
archive location.

## Privacy

See [`PRIVACY.md`](PRIVACY.md) for the full story. Short version: Recall
does not phone home, does not require an account, and ships with anonymous
crash reporting off by default.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

[MIT](LICENSE). Copyright (c) 2026 Recall contributors.