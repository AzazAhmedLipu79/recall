// Boot module — wires every view into the shell and runs the
// sequential startup sequence.
//
// IMPORTANT: modules are deferred, so this file's top-level code
// already runs after the parser finishes the body, but the explicit
// DOMContentLoaded listener below keeps the boot order greppable and
// lets us fail loudly if `__TAURI__` is missing.

import { $ } from "./util/dom.js";
import { toast } from "./util/toast.js";
import { invoke, convertFileSrc } from "./api.js";
import { mediaUrl } from "./util/url.js";
import { bindGlobalKeys } from "./util/keys.js";
import { bindNavButtons, showView } from "./router.js";

import { initStatusbar } from "./views/statusbar.js";
import { initLibrary } from "./views/library.js";
import { initAlbums } from "./views/albums.js";
import { initInspector } from "./views/inspector.js";
import { initLightbox } from "./views/lightbox.js";
import { initImport } from "./views/import.js";
import { initSettings } from "./views/settings.js";
import { initWizard } from "./views/wizard.js";
import { initAlbumMenu } from "./components/album-menu.js";
import { initShortcutOverlay } from "./components/shortcut-overlay.js";
import { initCrashReporter } from "./util/crash-reporter.js";
import { getSelectedId } from "./state.js";

function assertTauri() {
  if (!window.__TAURI__) {
    toast("Tauri runtime not available",
      "This window must be opened by the desktop app.", "err", 6000);
    throw new Error("__TAURI__ missing");
  }
}

async function boot() {
  assertTauri();

  // Shared mediaUrl binds invoke + convertFileSrc once so view modules
  // don't each grab the Tauri globals.
  const mediaUrlFn = (id) => mediaUrl(id, invoke, convertFileSrc);

  // Statusbar — used by import/sidebar/wizard so init first.
  const { refreshStatus } = initStatusbar();

  // Crash reporter — installed first so its global error handlers
  // capture issues from every subsequent init. Settings view owns the
  // toggle UI.
  const crashReporter = initCrashReporter();

  // Inspector / lightbox — they reference each other so build both
  // before wiring.
  let openInspectorRef = () => {};
  let openLightboxRef = () => {};
  const inspector = initInspector({
    mediaUrl: mediaUrlFn,
    openLightbox: () => openLightboxRef(),
  });
  openInspectorRef = inspector.openInspector;

  const lightbox = initLightbox({ mediaUrl: mediaUrlFn });
  openLightboxRef = lightbox.openLightbox;

  // Library needs openInspector; sidebar needs runSearch.
  const { runSearch } = initLibrary({
    invoke,
    mediaUrl: mediaUrlFn,
    openInspector: inspector.openInspector,
  });
  // Albums needs runSearch for the library side. It owns refreshAlbums
  // internally — Settings + Import get it back via the return value.
  const albumsApi = initAlbums({ runSearch });
  const { refreshAlbums } = albumsApi;
  albumsApi.setRefreshAlbums(refreshAlbums);

  // Settings + import — both need refreshStatus / refreshAlbums.
  const { refreshSettings } = initSettings({
    refreshStatus,
    refreshAlbums,
    runSearch,
    crashReporter,
  });
  initImport({ refreshStatus, refreshAlbums, showView });

  // Wizard — may auto-open if first run. The wizard module itself
  // persists `recall_wizard_seen` in tauri-plugin-store; these hooks
  // are for the rest of the app (route the user to Library).
  const wizard = initWizard({
    onFinish: () => { showView("library"); },
    runSearch,
    refreshStatus,
  });

  initAlbumMenu();
  const shortcuts = initShortcutOverlay();

  // Topbar nav buttons. Refresh hooks pass runSearch so re-entering
  // Library re-runs the current search.
  bindNavButtons((name) => showView(name, { runSearch, refreshSettings }));

  // Global keyboard shortcuts.
  bindGlobalKeys({
    showView,
    focusSearch: () => $("q")?.focus(),
    focusImport: () => { showView("import"); $("import-source")?.focus(); },
    openInspector: () => {
      const sel = getSelectedId();
      if (sel) openInspectorRef(sel);
    },
    showShortcutOverlay: shortcuts.open,
  });

  // Sequential boot: refreshAlbums and refreshStatus hit independent
  // commands and write to disjoint DOM regions, so they can run in
  // parallel. The wizard's first-run probe runs alongside the initial
  // showView — it only blocks UI by toggling the modal if it actually
  // needs to show, so we don't gate startup on it.
  await Promise.all([refreshAlbums(), refreshStatus()]);
  // Pass refreshHooks on the initial showView so Library fires its
  // first search immediately. Without these, the view shows an empty
  // grid until the user navigates elsewhere and back.
  showView("library", { runSearch, refreshSettings });
  wizard.checkFirstRun();
}

document.addEventListener("DOMContentLoaded", () => {
  boot().catch((e) => {
    console.error("boot failed:", e);
    toast("Startup error", String(e?.message ?? e), "err", 8000);
  });
});