// Import view: drag-drop, folder picker stub, progress UI.
//
// Drag-drop uses Tauri 2's webview onDragDropEvent because the HTML5
// dataTransfer API is unreliable for folder paths in webviews.
//
// `refreshStatus` and `refreshAlbums` are passed in by main.js —
// called after a successful import so the sidebar / status bar update.

import { $ } from "../util/dom.js";
import { toast } from "../util/toast.js";
import { unwrapEvent, matchType } from "../util/channel.js";
import {
  Channel,
  getCurrentWebview,
  cmdImportFolder, cmdImportTakeout,
} from "../api.js";
import { importMode, setImportMode } from "../state.js";

let _refreshStatus = () => {};
let _refreshAlbums = () => {};
let _showView = () => {};

const dropzone = $("dropzone");
const dropzonePath = $("dropzone-path");
const importSourceInput = $("import-source");

function setImportSource(path) {
  importSourceInput.value = path;
  if (path) {
    dropzonePath.textContent = path;
    dropzonePath.classList.remove("hidden");
  } else {
    dropzonePath.classList.add("hidden");
    dropzonePath.textContent = "";
  }
}

function bindModeButtons() {
  document.querySelectorAll("#import-mode button").forEach((b) => {
    b.addEventListener("click", () => {
      setImportMode(b.dataset.mode);
      document.querySelectorAll("#import-mode button").forEach((x) =>
        x.classList.toggle("active", x === b));
    });
  });
}

function bindDropzone() {
  if (getCurrentWebview) {
    try {
      getCurrentWebview().onDragDropEvent((event) => {
        const p = event.payload || {};
        if (p.type === "enter" || p.type === "over") {
          dropzone.classList.add("hover");
        } else if (p.type === "leave") {
          dropzone.classList.remove("hover");
        } else if (p.type === "drop") {
          dropzone.classList.remove("hover");
          const paths = (p.paths || []).filter((x) => typeof x === "string" && x.length);
          if (!paths.length) {
            toast("Drop ignored", "Nothing usable was dropped.", "err");
            return;
          }
          setImportSource(paths[0]);
          if (paths.length > 1) {
            toast("Multiple paths dropped",
              `Using ${paths[0]} (other ${paths.length - 1} ignored).`, "info");
          }
          _showView("import");
          toast("Folder captured", paths[0], "info");
        }
      });
    } catch (e) {
      console.warn("onDragDropEvent unavailable:", e);
    }
  }
  // Click the dropzone → focus the path input so the user can paste/type.
  dropzone.addEventListener("click", () => importSourceInput.focus());
}

function bindPickButton() {
  // No tauri-plugin-dialog yet — typing the path is the only way to
  // pick. M5/M6 will swap this for a native folder picker.
  $("btn-pick-folder").addEventListener("click", () => {
    importSourceInput.focus();
    toast("Pick a folder",
      "Type or paste the absolute folder path, or drag a folder onto the zone above.",
      "info", 4500);
  });
}

function appendProgressLine(s) {
  const log = $("progress-log");
  log.textContent += s + "\n";
  log.scrollTop = log.scrollHeight;
}

function updateProgress(startedAt, total, done) {
  if (total > 0) {
    const pct = Math.min(100, Math.round((done / total) * 100));
    $("progress-fill").style.width = pct + "%";
    const elapsed = (Date.now() - startedAt) / 1000;
    const rate = done / Math.max(elapsed, 0.001);
    const remaining = rate > 0 ? (total - done) / rate : 0;
    $("stat-eta").textContent = remaining > 0 ? `${remaining.toFixed(0)}s` : "–";
  } else {
    $("progress-fill").style.width = "30%"; // indeterminate until total arrives
  }
}

function bindRunImport() {
  $("run-import").addEventListener("click", async () => {
    const source = importSourceInput.value.trim();
    if (!source) { toast("No source", "Enter or drop a folder path first.", "err"); return; }
    $("import-progress").classList.remove("hidden");
    $("progress-fill").style.width = "0%";
    $("stat-imported").textContent = "0";
    $("stat-dup").textContent = "0";
    $("stat-failed").textContent = "0";
    $("stat-eta").textContent = "–";
    $("progress-log").textContent = `Starting ${importMode} import of ${source}…\n`;
    if (!Channel) { toast("Channel API missing", "Tauri runtime unavailable.", "err"); return; }
    const channel = new Channel();
    const startedAt = Date.now();
    let total = 0, done = 0, imported = 0, dup = 0, failed = 0;
    channel.onmessage = (ev) => {
      const e = unwrapEvent(ev);
      appendProgressLine(JSON.stringify(e));
      if (e.type === "started" && typeof e.total === "number") {
        total = e.total || 0;
        updateProgress(startedAt, total, done);
      } else if (matchType(e, "file_imported", "fileImported")) {
        imported++; done++;
        $("stat-imported").textContent = imported;
        updateProgress(startedAt, total, done);
      } else if (matchType(e, "file_duplicate", "fileDuplicate")) {
        dup++; done++;
        $("stat-dup").textContent = dup;
        updateProgress(startedAt, total, done);
      } else if (matchType(e, "file_failed", "fileFailed")) {
        failed++; done++;
        $("stat-failed").textContent = failed;
        updateProgress(startedAt, total, done);
      } else if (e.type === "finished") {
        $("progress-fill").style.width = "100%";
      }
    };
    try {
      const cmd = importMode === "takeout" ? cmdImportTakeout : cmdImportFolder;
      const args = importMode === "takeout"
        ? { source, channel }
        : { source, label: $("import-label").value || null, channel };
      const summary = await cmd(args);
      $("progress-fill").style.width = "100%";
      toast("Import complete",
        `${summary.imported} imported, ${summary.duplicates} dup, ${summary.failed} failed`,
        summary.failed > 0 ? "info" : "ok");
      _refreshStatus();
      _refreshAlbums();
    } catch (e) {
      toast("Import failed", String(e), "err");
    }
  });
}

export function initImport({ refreshStatus, refreshAlbums, showView }) {
  _refreshStatus = refreshStatus ?? (() => {});
  _refreshAlbums = refreshAlbums ?? (() => {});
  _showView = showView ?? (() => {});
  bindModeButtons();
  bindDropzone();
  bindPickButton();
  bindRunImport();
}