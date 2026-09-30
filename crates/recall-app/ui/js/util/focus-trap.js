// Focus-trap utility for modal overlays.
//
// `trapFocus(container, { onClose })`:
//   - On call, records `document.activeElement` so we can restore it
//     when the overlay closes (e.g. a tile that opened the inspector).
//   - Moves focus to the first focusable element inside the container,
//     or the container itself if nothing is focusable.
//   - Intercepts Tab/Shift+Tab to cycle within the container.
//   - Intercepts Escape to call onClose().
//   - Returns `release()` which removes the listeners and restores the
//     previously-active element to focus.
//
// This is intentionally minimal — we don't synthesize tabindexes on
// every element, just constrain Tab navigation among real focusables.

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type='hidden'])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

function focusables(container) {
  return Array.from(container.querySelectorAll(FOCUSABLE))
    .filter((el) => el.offsetParent !== null || el === document.activeElement);
}

export function trapFocus(container, { onClose } = {}) {
  if (!container) return () => {};
  const previouslyFocused = document.activeElement;
  const list = focusables(container);
  const first = list[0] ?? container;
  const last = list[list.length - 1] ?? container;
  // Focus the first interactive element; fall back to the container.
  if (typeof first.focus === "function") first.focus();
  if (container.getAttribute("tabindex") === null && first === container) {
    container.setAttribute("tabindex", "-1");
  }

  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose?.();
      return;
    }
    if (e.key !== "Tab") return;
    const current = focusables(container);
    if (!current.length) {
      e.preventDefault();
      return;
    }
    const firstFocusable = current[0];
    const lastFocusable = current[current.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && active === firstFocusable) {
      e.preventDefault();
      lastFocusable.focus();
    } else if (!e.shiftKey && active === lastFocusable) {
      e.preventDefault();
      firstFocusable.focus();
    }
  }

  container.addEventListener("keydown", onKey);
  return function release() {
    container.removeEventListener("keydown", onKey);
    if (previouslyFocused && typeof previouslyFocused.focus === "function") {
      previouslyFocused.focus();
    }
  };
}