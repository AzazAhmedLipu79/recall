// Minimal view router. Toggles `.hidden` on the view sections and
// `.active` on the matching topbar button. View-specific refresh
// hooks are passed in by main.js so this stays view-agnostic.

import { $ } from "./util/dom.js";

const VIEWS = ["library", "import", "settings"];

export function bindNavButtons(onShow) {
  document.querySelectorAll(".topbar nav button").forEach((b) => {
    b.addEventListener("click", () => onShow(b.id.replace("btn-", "")));
  });
}

export function showView(name, refreshHooks = {}) {
  VIEWS.forEach((v) => {
    const el = $(`view-${v}`);
    if (el) el.classList.toggle("hidden", v !== name);
  });
  document.querySelectorAll(".topbar nav button").forEach((b) => {
    b.classList.toggle("active", b.id === `btn-${name}`);
  });
  // Per-view refresh: settings wants to reload archive info each time
  // it's shown; library wants to re-run the current search.
  if (name === "settings" && refreshHooks.refreshSettings) refreshHooks.refreshSettings();
  if (name === "library"  && refreshHooks.runSearch)        refreshHooks.runSearch();
}