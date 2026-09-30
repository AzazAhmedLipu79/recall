# Privacy

## TL;DR

Recall runs entirely on your machine. It does not upload your photos, does
not require an account, and does not collect usage analytics by default.

## What Recall stores on your computer

Everything lives under the **archive folder** you choose on first launch
(default: `~/Recall` on macOS). That folder contains:

- `archive.db` — a local SQLite database with metadata (filenames,
  timestamps, EXIF, GPS, album assignments). No file contents.
- `store/objects/...` — the original files, stored by SHA-256 hash.
- `store/sidecars/...` — original Takeout sidecar JSON files where present.
- `logs/recall.log.YYYY-MM-DD` — runtime logs (one file per day, retained
  for 30 days).

Deleting the folder deletes your archive. There is no cloud copy.

## What Recall does *not* do

- It does not connect to any server, ever, unless you enable the optional
  crash report (see below).
- It does not index your machine outside the folder you point it at.
- It does not read your contacts, calendar, location services, or any
  other system data.
- It does not bundle third-party trackers, ads, or analytics SDKs.

## Crash reports (opt-in)

If you enable **Settings → Privacy → Send anonymous crash reports**, Recall
will send the following to a self-hosted Sentry-compatible endpoint:

- A stack trace if the app crashes.
- The macOS version and Recall version.
- No filenames, no thumbnails, no archive contents, no paths.

You can turn this off at any time; when off, **no network calls leave your
machine**. The setting is stored locally in your archive folder.

## What you should know before enabling crash reports

- Even with anonymisation, the **stack trace itself can contain file
  paths** (e.g. `/Users/alice/Photos/vacation/beach.jpg`). Recall strips
  user-identifying path segments before sending. If you don't trust the
  stripping, leave the setting off.

## Permissions the app requests

- **Full Disk Access** — only if you point the app at a folder outside your
  user home (e.g. an external drive). Without this permission, the app can
  still read from `~/Pictures`, `~/Downloads`, etc.
- **Network** — only if you enable crash reports, and only to the Sentry
  endpoint above. The app never opens a listening socket.

## Where to ask questions

Open an issue on GitHub. For sensitive disclosures, email
`security@recall.app` (replace with your real address before launch).