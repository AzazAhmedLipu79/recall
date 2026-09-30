# Contributing to Recall

Thanks for being here. Recall is a small project and good first issues are
genuinely the fastest way to get familiar with the codebase.

## Where to start

- **Issues labelled [`good first issue`](https://github.com/recall-app/recall/issues?q=is%3issue+is%3open+label%3A%22good+first+issue%22)**
  are scoped, single-file changes with clear acceptance criteria.
- **Issues labelled [`help wanted`](https://github.com/recall-app/recall/issues?q=is%3issue+is%3open+label%3A%22help+wanted%22)**
  are bigger — reach out before starting so we can coordinate.
- Anything else: open an issue first. Even a one-liner bug should have an
  issue so we can confirm it's actually a bug and not by-design.

## Development setup

You need:

- **Rust 1.77+** (`rustup update stable`).
- **macOS 13+** with Xcode Command Line Tools (`xcode-select --install`).
- **Node-free.** The frontend is vanilla HTML + ES modules. No bundler.

Then:

```sh
git clone https://github.com/recall-app/recall
cd recall
cargo build --release -p recall-app
RECALL_ARCHIVE_ROOT=$HOME/recall-dev ./target/release/recall-app
```

To iterate on the UI without rebuilding Rust:

```sh
# Terminal 1
cargo tauri dev --manifest-path crates/recall-app/Cargo.toml
```

The first build is slow (Tauri pulls in WebKit bindings); subsequent builds
are incremental.

## Repository layout

```
crates/
  recall-core         # shared types (MediaId, ArchivePath, Progress trait)
  recall-hash         # SHA-256
  recall-storage      # content-addressed object store
  recall-db           # SQLite catalog + query layer (largest engine crate)
  recall-metadata     # EXIF + sidecar parsing
  recall-importer     # folder + takeout walkers
  recall-verify       # integrity checker
  recall-backup       # copy + zip export
  recallctl           # headless CLI (mirrors what recall-app does)
  recall-app          # Tauri 2 desktop UI
```

The engine crates know nothing about Tauri. **All webview code lives in
`crates/recall-app/src/commands.rs` and `crates/recall-app/ui/`.** If your
change touches the engine, add a test in the relevant crate.

## Code style

- **Rust**: `rustfmt` with default config, `clippy::pedantic` is a goal not
  a gate. Run `cargo fmt && cargo clippy` before sending a PR.
- **JavaScript**: Prettier with default config, 2-space indent. The UI is
  small enough that we don't need ESLint.
- **Commits**: short imperative subject ("add album rename command"), body
  explains the *why*. Squash noise.

## Testing

```sh
# Engine unit tests (fast, no GUI)
cargo test --workspace --exclude recall-app

# CLI smoke test
cargo run -p recallctl -- init --archive-root /tmp/recall-test
cargo run -p recallctl -- seed --archive-root /tmp/recall-test --count 5 --with-takeout
cargo run -p recallctl -- status --archive-root /tmp/recall-test
cargo run -p recallctl -- verify --archive-root /tmp/recall-test
```

## Release process (maintainers)

1. Bump `version` in the workspace `Cargo.toml`.
2. Update `CHANGELOG.md` (we use `git-cliff`).
3. Tag: `git tag -s v0.x.y -m "v0.x.y"`.
4. Push: `git push origin v0.x.y`.
5. GitHub Actions builds a signed + notarised `.dmg` and attaches it to the
   GitHub Release. The updater manifest is updated automatically.

Signing credentials live in the org's GitHub Actions secrets — see
`docs/maintainer-secrets.md` (not yet written; ask before your first
release).