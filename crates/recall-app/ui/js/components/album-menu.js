// Album-menu — dropdown showing all user albums + "Create new…" option.
// Used by the inspector's "Add to album…" trigger.
//
// Popover positioning: anchored to the trigger button via its
// getBoundingClientRect so it works regardless of where the trigger
// lives in the DOM.

import { $ } from "../util/dom.js";
import { toast } from "../util/toast.js";
import {
  cmdListAlbums, cmdCreateAlbum, cmdAddToAlbum,
} from "../api.js";

// Build the dropdown contents as a fresh DOM subtree each time it opens,
// so newly-created albums appear immediately.
async function renderDropdown(root, mediaId) {
  root.innerHTML = "";
  let albums;
  try {
    albums = await cmdListAlbums();
  } catch (e) {
    root.innerHTML = `<div class="small-muted">Couldn't load albums: ${e}</div>`;
    return;
  }
  const create = document.createElement("button");
  create.className = "album-menu-create";
  create.textContent = "Create new album…";
  create.addEventListener("click", async () => {
    const name = window.prompt("Album name?");
    if (!name || !name.trim()) return;
    try {
      const id = await cmdCreateAlbum(name.trim());
      await cmdAddToAlbum(id, mediaId);
      toast("Added to album", name.trim(), "ok");
      root.classList.add("hidden");
    } catch (e) {
      toast("Couldn't add to album", String(e), "err");
    }
  });
  root.appendChild(create);
  for (const a of albums) {
    const b = document.createElement("button");
    b.className = "album-menu-item";
    b.textContent = a.name;
    b.addEventListener("click", async () => {
      try {
        await cmdAddToAlbum(a.id, mediaId);
        toast("Added to album", a.name, "ok");
        root.classList.add("hidden");
      } catch (e) {
        toast("Couldn't add to album", String(e), "err");
      }
    });
    root.appendChild(b);
  }
}

function positionMenu(root, trigger) {
  const r = trigger.getBoundingClientRect();
  const w = root.getBoundingClientRect();
  // Default: open below the trigger, left-aligned.
  let top = r.bottom + 4;
  let left = r.left;
  // Flip up if it would overflow the viewport bottom.
  if (top + w.height > window.innerHeight) top = Math.max(4, r.top - w.height - 4);
  // Clamp horizontally so it stays on-screen.
  if (left + w.width > window.innerWidth) left = Math.max(4, window.innerWidth - w.width - 4);
  root.style.top = `${top}px`;
  root.style.left = `${left}px`;
}

let _currentTrigger = null;
function openAt(root, trigger, mediaId) {
  _currentTrigger = trigger;
  renderDropdown(root, mediaId);
  root.classList.remove("hidden");
  // Render so we can measure before positioning.
  requestAnimationFrame(() => positionMenu(root, trigger));
}

export function initAlbumMenu() {
  const root = $("album-menu");
  if (!root) return;
  document.addEventListener("click", (e) => {
    const trigger = e.target.closest("[data-action='open-album-menu']");
    if (trigger) {
      e.preventDefault();
      const mediaId = trigger.getAttribute("data-media-id");
      if (mediaId) openAt(root, trigger, mediaId);
    } else if (!root.contains(e.target)) {
      root.classList.add("hidden");
      _currentTrigger = null;
    }
  });
  // Reposition on scroll/resize so the popover stays anchored.
  window.addEventListener("scroll", () => {
    if (!root.classList.contains("hidden") && _currentTrigger) {
      positionMenu(root, _currentTrigger);
    }
  }, true);
  window.addEventListener("resize", () => {
    if (!root.classList.contains("hidden") && _currentTrigger) {
      positionMenu(root, _currentTrigger);
    }
  });
  return { openAlbumMenu: (mediaId, trigger) => openAt(root, trigger, mediaId) };
}