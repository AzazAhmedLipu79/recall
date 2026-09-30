// Library view: search form, results grid (grouped by year), and the
// IntersectionObserver that lazy-loads photo thumbnails.
//
// Public:
//   initLibrary({ invoke, mediaUrl, openInspector }) — wire up handlers.
//
// The IntersectionObserver is module-singleton. It re-observes any
// `.thumb-pending` placeholder after each search.

import { $, delegate } from "../util/dom.js";
import { toast } from "../util/toast.js";
import { makeTile } from "../components/tile.js";
import { cmdSearch } from "../api.js";
import {
  currentKind, setCurrentKind,
  currentAlbum, setCurrentAlbum,
  setCurrentResults,
} from "../state.js";

let _invoke = null;
let _mediaUrl = null;
let _openInspector = null;

let _resultsEl = null;
let _resultsMetaEl = null;
let _qEl = null;
let _hasLocEl = null;
let _limitEl = null;

let thumbObserver = null;

function ensureThumbObserver() {
  if (thumbObserver) return thumbObserver;
  thumbObserver = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      const ph = entry.target;
      thumbObserver.unobserve(ph);
      const id = ph.getAttribute("data-id");
      if (!id) continue;
      (async () => {
        const url = await _mediaUrl(id);
        if (!url || !ph.isConnected || !ph.parentNode) return;
        // Only swap if the placeholder is still pending — a re-render
        // may have replaced it already.
        if (!ph.classList.contains("thumb-pending")) return;
        const img = document.createElement("img");
        img.className = "thumb";
        img.loading = "lazy";
        img.decoding = "async";
        img.src = url;
        img.alt = "";
        img.onerror = () => { ph.textContent = "PHOTO"; };
        ph.replaceWith(img);
      })();
    }
  }, { rootMargin: "200px" });
  return thumbObserver;
}

function lazyLoadThumbnails() {
  const obs = ensureThumbObserver();
  for (const ph of document.querySelectorAll(".thumb-pending")) {
    obs.observe(ph);
  }
}

async function runSearch() {
  _resultsEl.innerHTML = `<div class="empty-state"><div>Searching…</div></div>`;
  _resultsMetaEl.textContent = "";
  const args = {
    kind: currentKind || null,
    text: _qEl.value || null,
    album: currentAlbum || null,
    has_location: _hasLocEl.checked || false,
    limit: parseInt(_limitEl.value, 10) || 200,
  };
  let rows;
  try {
    rows = await cmdSearch(args);
  } catch (e) {
    _resultsEl.innerHTML = "";
    toast("Search failed", String(e), "err");
    return;
  }
  setCurrentResults(rows || []);
  if (!rows || !rows.length) {
    _resultsEl.innerHTML = `<div class="empty-state">
      <div class="hint">No items match these filters.</div>
      <div class="small-muted">Try clearing the search box or selecting "All".</div>
    </div>`;
    _resultsMetaEl.textContent = "0 results";
    return;
  }
  _resultsMetaEl.textContent =
    `${rows.length} result${rows.length === 1 ? "" : "s"}`;
  _resultsEl.innerHTML = "";
  // Group by year for a friendlier timeline layout.
  const groups = {};
  for (const m of rows) {
    const yr = m.canonical_taken_at
      ? new Date(m.canonical_taken_at * 1000).getUTCFullYear()
      : "Unknown";
    (groups[yr] ||= []).push(m);
  }
  const years = Object.keys(groups).sort((a, b) => {
    if (a === "Unknown") return 1;
    if (b === "Unknown") return -1;
    return Number(b) - Number(a);
  });
  for (const yr of years) {
    const h = document.createElement("div");
    h.className = "year-header";
    h.textContent = yr;
    _resultsEl.appendChild(h);
    const grid = document.createElement("div");
    grid.className = "grid";
    for (const m of groups[yr]) grid.appendChild(makeTile(m, _openInspector));
    _resultsEl.appendChild(grid);
  }
  lazyLoadThumbnails();
}

function bindSidebarFilters() {
  delegate($("kind-filters"), ".nav-item", (li) => {
    setCurrentKind(li.dataset.kind ?? "");
    document.querySelectorAll("#kind-filters .nav-item").forEach((x) =>
      x.classList.toggle("active", x === li));
    runSearch();
  });
}

function bindSearchBar() {
  $("run-search").addEventListener("click", runSearch);
  $("q").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); runSearch(); }
  });
}

function bindAlbumPick() {
  $("album-pick").addEventListener("change", () => {
    setCurrentAlbum($("album-pick").value);
    runSearch();
  });
}

// Arrow-key navigation between tiles. We use the actual computed
// column count of the active grid (CSS auto-fill can produce anywhere
// from 1 to N columns depending on viewport width) so left/right stays
// horizontal and up/down moves a row.
function gridColumns(grid) {
  const tpl = getComputedStyle(grid).gridTemplateColumns;
  return tpl.split(" ").filter(Boolean).length || 1;
}

function tilesOfGrid(grid) {
  return Array.from(grid.querySelectorAll(".tile"));
}

function bindTileArrowKeys() {
  _resultsEl.addEventListener("keydown", (e) => {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key)) return;
    const tile = e.target.closest(".tile");
    if (!tile) return;
    const grid = tile.closest(".grid");
    if (!grid) return;
    const tiles = tilesOfGrid(grid);
    const idx = tiles.indexOf(tile);
    if (idx < 0) return;
    const cols = gridColumns(grid);
    let next = idx;
    if (e.key === "ArrowLeft")  next = idx - 1;
    if (e.key === "ArrowRight") next = idx + 1;
    if (e.key === "ArrowUp")    next = idx - cols;
    if (e.key === "ArrowDown")  next = idx + cols;
    if (next < 0 || next >= tiles.length) return;
    e.preventDefault();
    tiles[next].focus();
    // Mirror the visual selection so ⌘I / Enter still works.
    tiles[next].classList.add("selected");
    tiles.filter((t) => t !== tiles[next]).forEach((t) => t.classList.remove("selected"));
  });
}

export function initLibrary({ invoke, mediaUrl, openInspector }) {
  _invoke = invoke;
  _mediaUrl = mediaUrl;
  _openInspector = openInspector;
  // Cache singleton lookups so runSearch() doesn't re-query on every
  // keystroke-Enter.
  _resultsEl = $("results");
  _resultsMetaEl = $("results-meta");
  _qEl = $("q");
  _hasLocEl = $("has-loc");
  _limitEl = $("limit");
  bindSidebarFilters();
  bindSearchBar();
  bindAlbumPick();
  bindTileArrowKeys();
  return { runSearch };
}