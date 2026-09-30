// Lightbox — fullscreen preview with prev/next + keyboard nav.
//
// Public:
//   initLightbox({ mediaUrl }) — wire up handlers and keyboard nav.
//   openLightbox()              — open for the currently selected media.

import { $, escapeHtml } from "../util/dom.js";
import {
  currentResults,
  selectedId, setSelectedId,
} from "../state.js";
import { trapFocus } from "../util/focus-trap.js";

let _mediaUrl = null;
let _releaseTrap = null;

const lightbox = $("lightbox");
const lbFrame = $("lb-frame");
const lbCounter = $("lb-counter");
const lbClose = $("lb-close");
const lbPrev = $("lb-prev");
const lbNext = $("lb-next");

async function showLightbox(list, idx) {
  setSelectedId(list[idx].id);
  const row = list[idx];
  lbFrame.innerHTML = "";
  const url = await _mediaUrl(row.id);
  if (!url) {
    lbFrame.innerHTML = `<div class="placeholder">${escapeHtml(row.kind ?? "PHOTO")}</div>`;
  } else if (row.kind === "video") {
    const v = document.createElement("video");
    v.src = url;
    v.controls = true;
    v.autoplay = true;
    v.preload = "auto";
    lbFrame.appendChild(v);
  } else if (row.kind === "audio") {
    const a = document.createElement("audio");
    a.src = url;
    a.controls = true;
    a.autoplay = true;
    a.style.width = "60vw";
    lbFrame.appendChild(a);
  } else if (row.kind === "document" && row.mime_type === "application/pdf") {
    const iframe = document.createElement("iframe");
    iframe.src = url;
    iframe.title = row.original_filename ?? "PDF";
    iframe.className = "lb-pdf";
    lbFrame.appendChild(iframe);
  } else {
    const img = document.createElement("img");
    img.src = url;
    img.alt = row.original_filename ?? "";
    lbFrame.appendChild(img);
  }
  lbCounter.textContent = `${idx + 1} / ${list.length}`;
  lbPrev.disabled = idx <= 0;
  lbNext.disabled = idx >= list.length - 1;
  lightbox.classList.add("visible");
  if (_releaseTrap) _releaseTrap();
  _releaseTrap = trapFocus(lightbox, { onClose: closeLightbox });
}

function openLightbox() {
  if (!selectedId) return;
  // Build the navigation list: prefer the current search results so
  // arrow keys feel like scrubbing through the timeline.
  const navList = currentResults.length
    ? currentResults
    : [{ id: selectedId }];
  let idx = navList.findIndex((r) => r.id === selectedId);
  if (idx < 0) idx = 0;
  showLightbox(navList, idx);
}

function closeLightbox() {
  lightbox.classList.remove("visible");
  const v = lbFrame.querySelector("video, audio");
  if (v) v.pause();
  lbFrame.innerHTML = "";
  if (_releaseTrap) { _releaseTrap(); _releaseTrap = null; }
}

function lightboxStep(delta) {
  if (!currentResults.length) return;
  const idx = currentResults.findIndex((r) => r.id === selectedId);
  if (idx < 0) return;
  const next = idx + delta;
  if (next < 0 || next >= currentResults.length) return;
  showLightbox(currentResults, next);
}

// Container-scoped key handler so it only fires when the lightbox has
// focus (the focus-trap utility now owns Esc→close).
function bindKeyboard() {
  lightbox.addEventListener("keydown", (e) => {
    if (e.key === "ArrowLeft") { lightboxStep(-1); }
    else if (e.key === "ArrowRight") { lightboxStep(1); }
    else if (e.key === " ") {
      const v = lbFrame.querySelector("video, audio");
      if (v) { e.preventDefault(); if (v.paused) v.play(); else v.pause(); }
    }
  });
}

export function initLightbox({ mediaUrl }) {
  _mediaUrl = mediaUrl;
  lbClose.addEventListener("click", closeLightbox);
  lbPrev.addEventListener("click", () => lightboxStep(-1));
  lbNext.addEventListener("click", () => lightboxStep(1));
  lightbox.addEventListener("click", (e) => {
    if (e.target === lightbox) closeLightbox();
  });
  bindKeyboard();
  return { openLightbox, closeLightbox };
}