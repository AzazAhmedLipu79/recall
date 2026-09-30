// Tile factory — one element per result row.
//
// Each tile gets a deterministic aspect ratio (picked from a hash of
// the media id) so the grid looks like Google Photos without doing
// real masonry. Photo tiles render a "PHOTO" placeholder that the
// library view's IntersectionObserver swaps for an <img> when it
// scrolls into view.

import { escapeHtml, hashStr } from "../util/dom.js";
import { fmtDate } from "../util/format.js";

export function makeTile(m, onClick) {
  const tile = document.createElement("div");
  tile.className = "tile";
  tile.dataset.id = m.id;
  tile.tabIndex = 0;
  tile.setAttribute("role", "button");
  tile.setAttribute("aria-label", m.original_filename ?? m.archive_path ?? "media");

  // Packed-tile aspect ratio. Mix of square / portrait / landscape
  // gives the Google-Photos "rows of varying height" feel.
  const bucket = hashStr(m.id) % 6;
  let ratio = 1.0;
  if (bucket === 0)      ratio = 1.5;   // landscape
  else if (bucket === 1) ratio = 0.75;  // portrait
  else if (bucket === 2) ratio = 1.0;   // square
  else if (bucket === 3) ratio = 1.33;  // 4:3
  else if (bucket === 4) ratio = 0.8;   // 4:5 portrait
  // bucket 5 → 1.0 square (the default)
  tile.style.setProperty("--ratio", String(ratio));

  // Videos / audio get the kind label as a placeholder; photos get
  // a "PHOTO" pending placeholder that the lazy loader will swap.
  const isMedia = m.kind === "video" || m.kind === "audio";
  const initial = isMedia
    ? `<div class="placeholder">${escapeHtml(m.kind)}</div>`
    : `<div class="placeholder thumb-pending" data-id="${escapeHtml(m.id)}">PHOTO</div>`;

  const kindBadge = m.kind && m.kind !== "photo"
    ? `<span class="kind-badge">${escapeHtml(m.kind)}</span>`
    : "";

  tile.innerHTML = `
    ${initial}
    ${kindBadge}
    <div class="meta">
      <span class="name">${escapeHtml(m.original_filename ?? m.archive_path)}</span>
      <span class="date">${escapeHtml(fmtDate(m.canonical_taken_at ?? m.created_at))}</span>
    </div>`;

  tile.addEventListener("click", () => select());
  // Enter / Space → open inspector (single activation, like a button).
  tile.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      select();
    }
  });

  function select() {
    document.querySelectorAll(".tile.selected").forEach((t) => t.classList.remove("selected"));
    tile.classList.add("selected");
    onClick?.(m.id);
  }
  return tile;
}