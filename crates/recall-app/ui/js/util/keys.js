// Global keyboard shortcuts.
//
// The plan calls for ⌘1/⌘2/⌘3 to switch views, ⌘F to focus search,
// ⌘O to jump to Import, ⌘I to open inspector for the selected tile,
// ⌫ to close overlays, ←/→ and Space in the lightbox, and ? to show
// a shortcut overlay.
//
// `bindGlobalKeys` is called once from main.js after views init.
// It dispatches to handlers passed in by main — keeps this module
// view-agnostic.

export function bindGlobalKeys(handlers) {
  document.addEventListener("keydown", (e) => {
    // ⌘1 — Library
    if (e.metaKey && !e.shiftKey && !e.altKey && e.key === "1") {
      e.preventDefault();
      handlers.showView?.("library");
      return;
    }
    // ⌘2 — Import
    if (e.metaKey && !e.shiftKey && !e.altKey && e.key === "2") {
      e.preventDefault();
      handlers.showView?.("import");
      return;
    }
    // ⌘3 — Settings
    if (e.metaKey && !e.shiftKey && !e.altKey && e.key === "3") {
      e.preventDefault();
      handlers.showView?.("settings");
      return;
    }
    // ⌘F — focus search
    if (e.metaKey && !e.shiftKey && !e.altKey && e.key === "f") {
      e.preventDefault();
      handlers.focusSearch?.();
      return;
    }
    // ⌘O — jump to import + focus path input
    if (e.metaKey && !e.shiftKey && !e.altKey && e.key === "o") {
      e.preventDefault();
      handlers.focusImport?.();
      return;
    }
    // ⌘I — inspector for selected tile
    if (e.metaKey && !e.shiftKey && !e.altKey && e.key === "i") {
      e.preventDefault();
      handlers.openInspector?.();
      return;
    }
    // ? — show shortcut overlay (shift+/ on US layouts)
    if (e.key === "?" && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      handlers.showShortcutOverlay?.();
      return;
    }
  });
}