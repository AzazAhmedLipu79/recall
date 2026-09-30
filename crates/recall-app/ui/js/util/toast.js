// Toast notifications — zero deps aside from the `toasts` element in
// index.html and the escapeHtml helper.
//
// Usage:
//   import { toast } from "./util/toast.js";
//   toast("Saved", "Settings updated", "ok");
//
// `kind` is one of "info" (default), "ok", "err" — colours the left border.
// If the toast stack element isn't in the DOM yet, toast() is a no-op
// rather than throwing — useful during early boot.

import { $, escapeHtml } from "./dom.js";

export function toast(title, body, kind = "info", timeout = 3500) {
  const stack = $("toasts");
  if (!stack) return; // no-op if shell not mounted yet
  const t = document.createElement("div");
  t.className = `toast ${kind}`;
  t.innerHTML = `<div class="title">${escapeHtml(title)}</div>` +
    (body ? `<div class="body">${escapeHtml(body)}</div>` : "");
  stack.appendChild(t);
  setTimeout(() => {
    t.style.opacity = "0";
    t.style.transition = "opacity 0.2s ease";
    setTimeout(() => t.remove(), 200);
  }, timeout);
}