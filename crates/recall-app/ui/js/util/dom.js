// DOM helpers — zero deps, pure DOM utilities.
// Kept as named exports so view modules can pull in what they need.

// Shorthand for document.getElementById.
export function $(id) {
  return document.getElementById(id);
}

// Escape a value for safe insertion into innerHTML. Handles null/undefined
// by producing an empty string. Matches the original behaviour.
export function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[c]));
}

// FNV-1a 32-bit hash. Used by the library view to pick a deterministic
// packed-tile aspect ratio per media id (so the grid feels varied but
// stable across renders).
export function hashStr(s) {
  let h = 2166136261 >>> 0;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619) >>> 0;
  }
  return h >>> 0;
}

// Delegated-event helper: fire `handler(target)` for any click whose
// target matches `selector` under `root`. Avoids re-binding on every
// re-render.
export function delegate(root, selector, handler) {
  root.addEventListener("click", (e) => {
    const target = e.target.closest(selector);
    if (target && root.contains(target)) {
      handler(target, e);
    }
  });
}

// Build a `<dl>` body from [[label, value], ...] pairs. Used by the
// inspector for File / EXIF / Timestamps / Source-files blocks.
export function dlRows(rows) {
  return rows.map(([k, v]) => `<dt>${escapeHtml(k)}</dt><dd>${escapeHtml(v)}</dd>`).join("");
}