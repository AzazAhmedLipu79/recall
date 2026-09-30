// Shortcut overlay — opened by `?`, lists every keyboard shortcut
// in the app. Esc and the × button dismiss.

import { $ } from "../util/dom.js";
import { trapFocus } from "../util/focus-trap.js";

let _releaseTrap = null;

function close() {
  $("shortcut-overlay")?.classList.add("hidden");
  if (_releaseTrap) { _releaseTrap(); _releaseTrap = null; }
}

function open() {
  const root = $("shortcut-overlay");
  if (!root) return;
  root.classList.remove("hidden");
  if (_releaseTrap) _releaseTrap();
  _releaseTrap = trapFocus(root, { onClose: close });
}

export function initShortcutOverlay() {
  const root = $("shortcut-overlay");
  if (!root) return { open, close };
  // Backdrop click closes.
  root.addEventListener("click", (e) => {
    if (e.target === root) close();
  });
  // Close button.
  root.addEventListener("click", (e) => {
    if (e.target.closest('[data-action="shortcut-close"]')) close();
  });
  return { open, close };
}