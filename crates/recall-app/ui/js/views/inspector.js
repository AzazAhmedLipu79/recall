// Inspector — slide-in side panel showing media detail.
//
// Public:
//   initInspector({ mediaUrl, openLightbox }) — wire up close handlers
//   openInspector(id)                          — open for the given media id

import { $, escapeHtml, dlRows } from "../util/dom.js";
import { toast } from "../util/toast.js";
import { fmtDate, fmtSize } from "../util/format.js";
import { cmdGetMediaDetail } from "../api.js";
import { setSelectedId } from "../state.js";
import { trapFocus } from "../util/focus-trap.js";

let _mediaUrl = null;
let _openLightbox = null;
let _releaseTrap = null;

const inspector = $("inspector");
const inspectorBackdrop = $("inspector-backdrop");

function closeInspector() {
  inspector.classList.remove("visible");
  inspectorBackdrop.classList.remove("visible");
  setSelectedId(null);
  document.querySelectorAll(".tile.selected").forEach((t) => t.classList.remove("selected"));
  // Pause any playing media so it doesn't keep streaming in the background.
  const v = inspector.querySelector("video, audio");
  if (v) v.pause();
  if (_releaseTrap) { _releaseTrap(); _releaseTrap = null; }
}

async function openInspector(id) {
  setSelectedId(id);
  inspector.classList.add("visible");
  inspectorBackdrop.classList.add("visible");
  $("ins-title").textContent = id;
  $("ins-preview").innerHTML = `<div class="ph">Loading…</div>`;
  ["file", "exif", "times", "albums", "locs", "sources"].forEach((k) => {
    const el = $(`ins-${k}`);
    if (el) el.innerHTML = "";
  });
  // Stash the current media id on the album-menu trigger so the popover
  // knows which item to add.
  const addBtn = $("ins-add-album");
  if (addBtn) addBtn.setAttribute("data-media-id", id);

  let detail;
  try {
    detail = await cmdGetMediaDetail(id);
  } catch (e) {
    toast("Failed to load item", String(e), "err");
    closeInspector();
    return;
  }
  if (!detail) {
    closeInspector();
    toast("Not found", `No media with id ${id}`, "err");
    return;
  }
  renderInspectorPreview(detail.media);
  renderInspectorMetadata(detail);

  // Click-to-expand: for photos/videos/audio, clicking the preview opens
  // the lightbox. PDFs already render their own scrollable viewer inside
  // the iframe, so the inspector preview is the final view — no
  // lightbox hop.
  const preview = $("ins-preview");
  if (detail.media.kind === "document" && detail.media.mime_type === "application/pdf") {
    preview.style.cursor = "default";
    preview.title = "";
  } else {
    preview.onclick = () => _openLightbox?.();
  }

  // Trap focus + Esc-to-close. Released when the inspector closes.
  if (_releaseTrap) _releaseTrap();
  _releaseTrap = trapFocus(inspector, { onClose: closeInspector });
}

async function renderInspectorPreview(m) {
  const url = await _mediaUrl(m.id);
  const kind = m.kind;
  const preview = $("ins-preview");
  preview.innerHTML = "";
  if (!url) {
    preview.innerHTML = `<div class="placeholder">${escapeHtml(kind ?? "PHOTO")}</div>`;
    return;
  }
  if (kind === "video") {
    const v = document.createElement("video");
    v.src = url;
    v.controls = true;
    v.preload = "metadata";
    v.title = "Click to open full-screen";
    preview.appendChild(v);
  } else if (kind === "audio") {
    const a = document.createElement("audio");
    a.src = url;
    a.controls = true;
    a.preload = "metadata";
    preview.appendChild(a);
  } else if (kind === "photo") {
    const img = document.createElement("img");
    img.src = url;
    img.alt = m.original_filename ?? "photo";
    img.title = "Click to open full-screen";
    preview.appendChild(img);
  } else if (kind === "document" || kind === "other") {
    // PDFs render natively in WKWebView; everything else falls back
    // to a labeled placeholder (no native preview for txt/docx/etc).
    if (m.mime_type === "application/pdf") {
      const iframe = document.createElement("iframe");
      iframe.src = url;
      iframe.title = m.original_filename ?? "PDF";
      iframe.className = "pdf-frame";
      preview.appendChild(iframe);
    } else {
      const ph = document.createElement("div");
      ph.className = "ph";
      ph.textContent = m.kind || "file";
      preview.appendChild(ph);
    }
  } else {
    const ph = document.createElement("div");
    ph.className = "ph";
    ph.textContent = "no preview";
    preview.appendChild(ph);
  }
}

function renderInspectorMetadata(detail) {
  const f = detail.media;
  $("ins-title").textContent = f.original_filename || f.archive_path;
  $("ins-file").innerHTML = dlRows([
    ["ID", f.id],
    ["Kind", f.kind],
    ["SHA-256", f.sha256],
    ["Size", fmtSize(f.size_bytes)],
    ["MIME", f.mime_type ?? "—"],
    ["Archive path", f.archive_path],
    ["Original filename", f.original_filename ?? "—"],
    ["Source label", f.source_label ?? "—"],
    ["Taken at", fmtDate(f.canonical_taken_at)],
    ["Created at", fmtDate(f.created_at)],
    ["Modified at", fmtDate(f.modified_at)],
  ]);

  if (detail.metadata) {
    const m = detail.metadata;
    $("ins-exif").innerHTML = dlRows([
      ["Camera", [m.camera_make, m.camera_model].filter(Boolean).join(" ") || "—"],
      ["Lens", m.lens ?? "—"],
      ["Focal length", m.focal_length ?? "—"],
      ["Aperture", m.aperture ?? "—"],
      ["Shutter", m.shutter_speed ?? "—"],
      ["ISO", m.iso ?? "—"],
      ["Dimensions", (m.width && m.height) ? `${m.width} × ${m.height}` : "—"],
      ["Orientation", m.orientation ?? "—"],
      ["Duration (s)", m.duration_seconds ?? "—"],
      ["Description", m.description ?? "—"],
    ]);
  } else {
    $("ins-exif").innerHTML = `<dt>—</dt><dd>No metadata extracted.</dd>`;
  }

  if (detail.timestamps.length) {
    $("ins-times").innerHTML = detail.timestamps.map((t) =>
      `<dt>${escapeHtml(t.source)}${t.label ? ` (${escapeHtml(t.label)})` : ""}</dt>` +
      `<dd>${escapeHtml(fmtDate(t.value))} <span class="small-muted">(${t.value})</span></dd>`).join("");
  } else {
    $("ins-times").innerHTML = `<dt>—</dt><dd>No provenance timestamps.</dd>`;
  }

  $("ins-albums").innerHTML = detail.albums.length
    ? detail.albums.map((a) => `<span class="pill">${escapeHtml(a.name)}</span>`).join("")
    : `<span class="small-muted">Not in any album.</span>`;

  $("ins-locs").innerHTML = detail.locations.length
    ? detail.locations.map((l) =>
        `<span class="pill" title="source: ${escapeHtml(l.source)}">` +
        `${l.latitude.toFixed(4)}, ${l.longitude.toFixed(4)}</span>`).join("")
    : `<span class="small-muted">No location data.</span>`;

  if (detail.source_files.length) {
    $("ins-sources").innerHTML = detail.source_files.map((f) =>
      `<dt>${fmtSize(f.size_bytes)}</dt><dd>${escapeHtml(f.original_path)}</dd>`).join("");
  } else {
    $("ins-sources").innerHTML = `<dt>—</dt><dd>No source paths tracked.</dd>`;
  }
}

export function initInspector({ mediaUrl, openLightbox }) {
  _mediaUrl = mediaUrl;
  _openLightbox = openLightbox;
  $("ins-close").addEventListener("click", closeInspector);
  inspectorBackdrop.addEventListener("click", closeInspector);
  return { openInspector, closeInspector };
}