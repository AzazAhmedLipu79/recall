// Albums sidebar — populates the right-hand sidebar list and the
// search-bar "Any album" select. Owns the "+ New album" button and
// the right-click rename / delete context menu.

import { $, escapeHtml, delegate } from "../util/dom.js";
import { toast } from "../util/toast.js";
import { askText, confirmAction } from "../util/dialog.js";
import { cmdListAlbums, cmdCreateAlbum, cmdRenameAlbum, cmdDeleteAlbum, ALBUM_KIND } from "../api.js";
import { currentAlbum, setCurrentAlbum } from "../state.js";

let _bound = false;

function onAlbumListClick(target) {
  if (target.dataset.all !== undefined) {
    setCurrentAlbum("");
  } else {
    const name = target.dataset.albumName;
    if (!name) return;
    setCurrentAlbum(name);
  }
  $("album-pick").value = currentAlbum;
  // Toggle .active on the existing rows in place — no need to re-query.
  $("album-list").querySelectorAll(".nav-item").forEach((x) =>
    x.classList.toggle("active",
      (x.dataset.all !== undefined && currentAlbum === "") ||
      (x.dataset.albumName !== undefined && x.dataset.albumName === currentAlbum)));
  _runSearch();
}

function onAlbumListContextMenu(target, e) {
  if (target.dataset.all !== undefined) return;
  const id = target.dataset.albumId;
  const name = target.dataset.albumName;
  if (!id || !name) return;
  e.preventDefault();
  showAlbumContextMenu(e.clientX, e.clientY, { id, name });
}

// Module-singleton context menu — one floating element, hidden by default.
let _contextMenu = null;
function ensureContextMenu() {
  if (_contextMenu) return _contextMenu;
  _contextMenu = document.createElement("div");
  _contextMenu.className = "album-context-menu hidden";
  document.body.appendChild(_contextMenu);
  // Click anywhere else dismisses.
  document.addEventListener("click", () => _contextMenu?.classList.add("hidden"));
  return _contextMenu;
}

function showAlbumContextMenu(x, y, { id, name }) {
  const menu = ensureContextMenu();
  menu.innerHTML = `
    <button data-action="rename">Rename…</button>
    <button data-action="delete">Delete…</button>
  `;
  menu.classList.remove("hidden");
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;
  // Clamp so the menu stays on-screen once we know its rendered size.
  const { innerWidth, innerHeight } = window;
  const r = menu.getBoundingClientRect();
  if (r.right > innerWidth)  menu.style.left = `${Math.max(4, x - r.width)}px`;
  if (r.bottom > innerHeight) menu.style.top  = `${Math.max(4, y - r.height)}px`;

  menu.querySelector('[data-action="rename"]').addEventListener("click", async () => {
    menu.classList.add("hidden");
    const next = (await askText({
      title: "Rename album",
      message: "Rename album",
      defaultValue: name,
    }) || "").trim();
    if (!next || next === name) return;
    try {
      await cmdRenameAlbum(id, next);
      toast("Album renamed", next, "ok");
      // If this album was the current filter, keep the search aligned.
      if (currentAlbum === name) setCurrentAlbum(next);
      await _refreshAlbums();
      // Re-run search only when the current filter was actually renamed.
      if (currentAlbum === next) _runSearch();
    } catch (e) {
      toast("Couldn't rename album", String(e), "err");
    }
  });

  menu.querySelector('[data-action="delete"]').addEventListener("click", async () => {
    menu.classList.add("hidden");
    const ok = await confirmAction({
      title: "Delete album",
      message: `Delete album "${name}"? Media files will stay — only the album is removed.`,
      okLabel: "Delete",
    });
    if (!ok) return;
    const wasCurrent = currentAlbum === name;
    try {
      await cmdDeleteAlbum(id);
      toast("Album deleted", name, "ok");
      if (wasCurrent) setCurrentAlbum("");
      await _refreshAlbums();
      // The deleted album can only have affected the visible results if it
      // was the active filter. Anything else: nothing changed in the library.
      if (wasCurrent) _runSearch();
    } catch (e) {
      toast("Couldn't delete album", String(e), "err");
    }
  });
}

async function refreshAlbums() {
  let albums;
  try {
    albums = await cmdListAlbums();
  } catch (e) {
    console.warn("refreshAlbums:", e);
    return;
  }
  const ul = $("album-list");
  const rows = [`<li class="nav-item${currentAlbum === "" ? " active" : ""}" data-all><span>All albums</span><span class="count">${albums.length}</span></li>`];
  if (!albums.length) {
    rows.push(`<li class="empty">No albums yet.</li>`);
  } else {
    for (const a of albums) {
      const count = a.media_count ?? 0;
      const kindBadge = a.kind === ALBUM_KIND.GoogleTakeout
        ? ` <span class="kind-mini">takeout</span>`
        : "";
      rows.push(`<li class="nav-item${currentAlbum === a.name ? " active" : ""}"` +
                ` data-album-id="${escapeHtml(a.id)}"` +
                ` data-album-name="${escapeHtml(a.name)}"` +
                ` data-album-kind="${escapeHtml(a.kind)}">` +
                `<span>${escapeHtml(a.name)}${kindBadge}</span>` +
                `<span class="count">${count}</span></li>`);
    }
  }
  ul.innerHTML = rows.join("");
  const sel = $("album-pick");
  sel.innerHTML = `<option value="">Any album</option>` +
    albums.map((a) => `<option value="${escapeHtml(a.name)}">${escapeHtml(a.name)}</option>`).join("");
  sel.value = currentAlbum;
}

async function bindNewAlbum() {
  const btn = $("new-album-btn");
  if (!btn) return;
  btn.addEventListener("click", async () => {
    const name = (await askText({ title: "New album", message: "Album name?" }) || "").trim();
    if (!name) return;
    try {
      await cmdCreateAlbum(name);
      toast("Album created", name, "ok");
      await _refreshAlbums();
    } catch (e) {
      toast("Couldn't create album", String(e), "err");
    }
  });
}

// --- Module init: returns the wired hooks so main.js can pass them
//     into other modules without module-level mutable state. ---

let _runSearch = () => {};
let _refreshAlbums = () => {};
// main.js sets this after the module's own refreshAlbums is available
// so the module's internal callers (rename/delete/new) reach the right
// closure without a circular init arg.
export function setRefreshAlbums(fn) { _refreshAlbums = fn ?? (() => {}); }

export function initAlbums({ runSearch }) {
  if (typeof runSearch !== "function") throw new Error("initAlbums: runSearch is required");
  _runSearch = runSearch;
  // _refreshAlbums gets wired up after this call via setRefreshAlbums()
  // so the boot sequence in main.js can both init us AND receive our
  // own refresh hook without a circular arg.
  if (!_bound) {
    const root = $("album-list");
    delegate(root, ".nav-item", onAlbumListClick);
    root.addEventListener("contextmenu", (e) => {
      const target = e.target.closest(".nav-item");
      if (target) onAlbumListContextMenu(target, e);
    });
    _bound = true;
  }
  bindNewAlbum();
  return { refreshAlbums, setRefreshAlbums };
}
