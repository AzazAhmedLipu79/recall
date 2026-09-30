// Formatting helpers — zero deps.

// `secs` is a Unix timestamp in seconds (the SQL schema uses INTEGER
// epoch seconds). Returns YYYY-MM-DD in UTC, or "—" for falsy values.
export function fmtDate(secs) {
  if (!secs) return "—";
  return new Date(secs * 1000).toISOString().slice(0, 10);
}

// Human-readable byte count. `fmtSize` caps at GB (good for individual
// file sizes); `fmtBytes` adds TB (used by the export-progress UI for
// aggregate totals).
function fmtBytesImpl(b, units) {
  if (!b) return "0 B";
  let n = b, i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(n < 10 ? 1 : 0)} ${units[i]}`;
}

export function fmtSize(b) {
  return fmtBytesImpl(b, ["B", "KB", "MB", "GB"]);
}

export function fmtBytes(b) {
  return fmtBytesImpl(b, ["B", "KB", "MB", "GB", "TB"]);
}