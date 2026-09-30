// Settings view: archive info, verify, backup, export, recent imports.
//
// Each action button wires a Channel<ProgressEvent> so the UI gets
// per-file updates. Some Tauri 2 builds wrap the payload; we cover both.

import { $ } from "../util/dom.js";
import { toast } from "../util/toast.js";
import { fmtBytes, fmtSize } from "../util/format.js";
import { unwrapEvent, matchType } from "../util/channel.js";
import { getSetting } from "../util/store.js";
import { pickFolder } from "../util/dialog.js";
import { trapFocus } from "../util/focus-trap.js";
import {
  Channel,
  cmdArchiveInfo, cmdListImports, cmdRevealInFinder,
  cmdVerify, cmdBackup, cmdExportZip, cmdMigrateArchive,
  cmdSetArchiveRoot, cmdProbeArchiveRoot,
} from "../api.js";

const JS_ERROR_LOGGING_KEY = "recall_js_error_logging";

let _refreshStatus = () => {};
let _runSearch     = () => {};
let _refreshAlbums = () => {};

async function refreshSettings() {
  try {
    const info = await cmdArchiveInfo();
    $("archive-path-text").textContent = info.archive_root;
    $("archive-stats").textContent =
      `${fmtSize(info.disk_usage_bytes)} on disk · ${info.media_count} items` +
      (info.is_custom ? "" : " · default location");
  } catch (e) {
    $("archive-stats").textContent = "error: " + e;
  }
  try {
    const imports = await cmdListImports(10);
    $("imports-list").textContent = imports.length
      ? imports.map((r) =>
          `${r.id}  ${r.source_kind}  ${r.status}  ${r.imported}+${r.duplicates}-${r.failed}/${r.discovered}`).join("\n")
      : "(no imports yet)";
  } catch (e) {
    $("imports-list").textContent = "error: " + e;
  }
}

function bindArchiveCard() {
  $("archive-show")?.addEventListener("click", async () => {
    try {
      const info = await cmdArchiveInfo();
      await cmdRevealInFinder(info.archive_root);
    } catch (e) {
      toast("Couldn't reveal archive", String(e), "err");
    }
  });
  $("archive-move")?.addEventListener("click", () => {
    const panel = $("move-archive-panel");
    if (!panel) return;
    panel.classList.toggle("hidden");
    if (!panel.classList.contains("hidden")) {
      $("move-archive-dest").value = "";
    }
  });
  $("move-archive-cancel")?.addEventListener("click", () => {
    $("move-archive-panel")?.classList.add("hidden");
  });
  $("move-archive-pick")?.addEventListener("click", async () => {
    try {
      const picked = await pickFolder({
        title: "Choose the new archive location",
        defaultPath: $("move-archive-dest").value || undefined,
      });
      if (picked) $("move-archive-dest").value = picked;
    } catch (e) {
      toast("Couldn't open folder picker", String(e), "err");
    }
  });
  $("move-archive-run")?.addEventListener("click", runMoveArchive);
  $("archive-switch")?.addEventListener("click", runSwitchArchive);
}

let _releaseTrap = null;

// Reveal the move panel with a destination already filled in, then kick
// off the move. Used by both the Switch archive flow (when the picked
// folder is empty) and the modal's "Copy into" choice.
function seedMovePanelAndRun(dest) {
  const panel = $("move-archive-panel");
  if (panel) panel.classList.remove("hidden");
  $("move-archive-dest").value = dest;
  // Forced false — we just freshly picked this folder; there's no
  // pre-existing source to delete.
  $("move-archive-delete").checked = false;
  return runMoveArchive();
}

async function runSwitchArchive() {
  let picked;
  try {
    picked = await pickFolder({ title: "Choose an archive folder" });
  } catch (e) {
    toast("Couldn't open folder picker", String(e), "err");
    return;
  }
  if (!picked) return;

  let probe;
  try {
    probe = await cmdProbeArchiveRoot(picked);
  } catch (e) {
    toast("Couldn't probe folder", String(e), "err");
    return;
  }

  if (probe.is_current) {
    toast("Already open", "That folder is already your current archive.", "info", 2500);
    return;
  }

  if (probe.has_archive_db || probe.has_store) {
    openSwitchModal(probe);
  } else if (probe.is_empty) {
    seedMovePanelAndRun(probe.path);
    toast(
      "Folder ready for copying",
      "Hit Move archive to copy your current archive into the chosen folder.",
      "info", 4000,
    );
  } else {
    toast(
      "Folder not empty",
      "That folder has files but no archive.db — pick an empty folder or one that already contains a Recall archive.",
      "err", 5000,
    );
  }
}

function openSwitchModal(probe) {
  const modal = $("switch-archive-modal");
  if (!modal) return;
  $("switch-archive-path").textContent = probe.path;
  const pitch = $("switch-archive-pitch");
  if (pitch) {
    pitch.innerHTML = probe.has_archive_db
      ? `We found <code>archive.db</code> at this location. Do you want to <strong>open</strong> this archive, or <strong>copy</strong> your current archive into this folder?`
      : `This folder already contains Recall media (<code>store/</code>) but no <code>archive.db</code>. Pick <em>Open</em> if it used to be the destination of a prior migration; otherwise choose <em>Copy</em>.`;
  }
  modal.classList.remove("hidden");
  if (_releaseTrap) _releaseTrap();
  _releaseTrap = trapFocus(modal, { onClose: closeSwitchModal });
}

function closeSwitchModal() {
  if (_releaseTrap) { _releaseTrap(); _releaseTrap = null; }
  $("switch-archive-modal")?.classList.add("hidden");
}

function _switchTarget() {
  return $("switch-archive-path")?.textContent?.trim() || null;
}

async function commitSwitch() {
  const target = _switchTarget();
  closeSwitchModal();
  if (!target) return;
  try {
    await cmdSetArchiveRoot(target);
    toast("Archive switched", `Now using ${target}`, "ok");
  } catch (e) {
    toast("Couldn't switch archive", String(e), "err");
    return;
  }
  await refreshAfterArchiveChange();
}

function commitCopyIntoTarget() {
  const target = _switchTarget();
  closeSwitchModal();
  if (!target) return;
  seedMovePanelAndRun(target);
}

// One helper so the move flow's success path and the switch flow can
// both refresh library state without re-running each one if a previous
// step already did.
async function refreshAfterArchiveChange() {
  const tasks = [
    refreshSettings(),
    Promise.resolve(_refreshStatus()).catch(() => {}),
    Promise.resolve(_refreshAlbums()).catch(() => {}),
    Promise.resolve(_runSearch()).catch(() => {}),
  ];
  // allSettled so one failure doesn't discard the other results.
  await Promise.allSettled(tasks);
}

function bindSwitchArchiveModal() {
  $("switch-archive-cancel")?.addEventListener("click", closeSwitchModal);
  $("switch-archive-switch")?.addEventListener("click", commitSwitch);
  $("switch-archive-copy")?.addEventListener("click", commitCopyIntoTarget);
  // Click outside the dialog dismisses (trapFocus already handles Escape).
  $("switch-archive-modal")?.addEventListener("click", (e) => {
    if (e.target === e.currentTarget) closeSwitchModal();
  });
}

async function runMoveArchive() {
  const dest = $("move-archive-dest").value.trim();
  const deleteSource = Boolean($("move-archive-delete")?.checked);
  if (!dest) { toast("Move failed", "Choose a destination folder first.", "err"); return; }
  if (!Channel) { toast("Channel API missing", "", "err"); return; }

  // Confirm before destructive ops.
  if (deleteSource) {
    const ok = window.confirm(
      `Move the archive to:\n${dest}\n\nThe old archive will be DELETED after a verified copy.\n\nContinue?`,
    );
    if (!ok) return;
  }

  const runBtn  = $("move-archive-run");
  const cancel  = $("move-archive-cancel");
  const result  = $("move-archive-result");
  const progBox = $("move-archive-progress");
  const fill    = $("move-archive-fill");
  const filesEl = $("move-archive-files");
  const bytesEl = $("move-archive-bytes");

  runBtn.disabled = true;
  cancel.disabled = true;
  result.textContent = "running…";
  progBox.classList.remove("hidden");
  fill.style.width = "0%";
  filesEl.textContent = "0";
  bytesEl.textContent = "0";

  const channel = new Channel();
  let files = 0, bytes = 0;
  channel.onmessage = (ev) => {
    const e = unwrapEvent(ev);
    if (matchType(e, "migration_started", "migrationStarted")) {
      result.textContent = "copying…";
    } else if (matchType(e, "migration_file_copied", "migrationFileCopied")) {
      files += 1;
      bytes += e.bytes ?? 0;
      filesEl.textContent = files;
      bytesEl.textContent = fmtSize(bytes);
      // Indeterminate fill — we don't know the total until everything's copied.
      fill.style.width = Math.min(90, 10 + (files % 80)) + "%";
    } else if (matchType(e, "migration_finished", "migrationFinished")) {
      fill.style.width = "100%";
      result.textContent = `done · ${files} files · ${fmtSize(bytes)}` +
        (e.deleted_source ? " · source deleted" : "");
    } else if (matchType(e, "migration_failed", "migrationFailed")) {
      result.textContent = "failed: " + (e.reason ?? "unknown");
    }
  };
  try {
    const summary = await cmdMigrateArchive({
      newRoot: dest,
      deleteSource,
      channel,
    });
    toast(
      "Archive moved",
      `${summary.files_copied} files · ${fmtSize(summary.bytes_copied)}` +
        (deleteSource ? " · source deleted" : ""),
      "ok",
    );
    await Promise.all([
      refreshSettings(),
      Promise.resolve(_refreshStatus()).catch(() => {}),
      Promise.resolve(_refreshAlbums()).catch(() => {}),
      Promise.resolve(_runSearch()).catch(() => {}),
    ]);
    // Collapse the panel on success.
    setTimeout(() => $("move-archive-panel")?.classList.add("hidden"), 800);
  } catch (e) {
    toast("Move failed", String(e), "err");
    result.textContent = "error: " + e;
  } finally {
    runBtn.disabled = false;
    cancel.disabled = false;
  }
}

function bindVerify(refreshStatus) {
  $("run-verify").addEventListener("click", async () => {
    if (!Channel) { toast("Channel API missing", "", "err"); return; }
    $("verify-result").textContent = "running…";
    const channel = new Channel();
    let ok = 0, missing = 0, mismatched = 0;
    channel.onmessage = (ev) => {
      const e = unwrapEvent(ev);
      if (matchType(e, "verify_ok", "verifyOk")) ok++;
      else if (matchType(e, "verify_missing", "verifyMissing")) missing++;
      else if (matchType(e, "verify_mismatched", "verifyMismatched")) mismatched++;
    };
    try {
      const stats = await cmdVerify(channel);
      $("verify-result").textContent =
        `ok=${stats.files_ok} missing=${stats.files_missing} mismatched=${stats.files_mismatched}`;
      toast("Verify complete",
        `${stats.files_ok} ok · ${stats.files_missing} missing · ${stats.files_mismatched} mismatched`,
        stats.files_missing + stats.files_mismatched > 0 ? "info" : "ok");
      refreshStatus();
    } catch (e) {
      $("verify-result").textContent = "error: " + e;
      toast("Verify failed", String(e), "err");
    }
  });
}

function bindBackup() {
  $("run-backup").addEventListener("click", async () => {
    const dest = $("backup-dest").value.trim();
    if (!dest) { toast("Backup failed", "Enter a destination path first.", "err"); return; }
    if (!Channel) { toast("Channel API missing", "", "err"); return; }
    $("backup-result").textContent = "running…";
    const channel = new Channel();
    let copied = 0, skipped = 0, failed = 0;
    channel.onmessage = (ev) => {
      const e = unwrapEvent(ev);
      if (matchType(e, "backup_copied", "backupCopied")) copied++;
      else if (matchType(e, "backup_skipped", "backupSkipped")) skipped++;
      else if (matchType(e, "backup_failed", "backupFailed")) failed++;
    };
    try {
      const summary = await cmdBackup({ destination: dest, channel });
      $("backup-result").textContent =
        `copied=${summary.files_copied} skipped=${summary.files_skipped} failed=${summary.files_failed}`;
      toast("Backup complete",
        `${summary.files_copied} copied · ${summary.files_skipped} skipped · ${summary.files_failed} failed`,
        summary.files_failed > 0 ? "info" : "ok");
    } catch (e) {
      $("backup-result").textContent = "error: " + e;
      toast("Backup failed", String(e), "err");
    }
  });
}

function bindExport() {
  $("run-export").addEventListener("click", async () => {
    const dest = $("export-dest").value.trim();
    if (!dest) { toast("Export failed", "Enter a destination .zip path first.", "err"); return; }
    if (!dest.toLowerCase().endsWith(".zip")) {
      toast("Export failed", "Destination should end in .zip", "err"); return;
    }
    if (!Channel) { toast("Channel API missing", "", "err"); return; }
    $("export-result").textContent = "running…";
    const channel = new Channel();
    let copied = 0, failed = 0, lastBytes = 0;
    let lastEvent = "";
    channel.onmessage = (ev) => {
      const e = unwrapEvent(ev);
      const t = e.type ?? "";
      lastEvent = t;
      if (matchType(e, "export_copied", "exportCopied")) {
        copied++;
        lastBytes = e.bytes ?? lastBytes;
        $("export-result").textContent =
          `running… copied=${copied} failed=${failed}`;
      } else if (matchType(e, "export_failed", "exportFailed")) {
        failed++;
        $("export-result").textContent =
          `running… copied=${copied} failed=${failed}`;
      }
    };
    try {
      const summary = await cmdExportZip({ destination: dest, channel });
      $("export-result").textContent =
        `copied=${summary.files_copied} failed=${summary.files_failed} bytes=${fmtBytes(summary.bytes_written)}`;
      toast("Export complete",
        `${summary.files_copied} files · ${fmtBytes(summary.bytes_written)}`,
        summary.files_failed > 0 ? "info" : "ok");
    } catch (e) {
      $("export-result").textContent = `error: ${e} (last event: ${lastEvent})`;
      toast("Export failed", String(e), "err");
    }
  });
}

async function refreshDiagnostics() {
  const cb = $("js-error-logging");
  if (cb) cb.checked = Boolean(await getSetting(JS_ERROR_LOGGING_KEY, false));
}

function bindDiagnostics(crashReporter) {
  const cb = $("js-error-logging");
  if (cb) cb.addEventListener("change", async () => {
    await crashReporter.setEnabled(cb.checked);
    toast(
      cb.checked ? "JS error logging on" : "JS error logging off",
      cb.checked
        ? "Future errors will be appended to logs/js-errors.log."
        : "Future errors will be ignored.",
      "info",
      3000,
    );
  });
  $("reveal-logs")?.addEventListener("click", () => crashReporter.revealLogsFolder());
}

export function initSettings({ refreshStatus, refreshAlbums, runSearch, crashReporter }) {
  _refreshStatus = refreshStatus ?? (() => {});
  _refreshAlbums = refreshAlbums ?? (() => {});
  _runSearch     = runSearch ?? (() => {});
  bindArchiveCard();
  bindSwitchArchiveModal();
  bindVerify(_refreshStatus);
  bindBackup();
  bindExport();
  if (crashReporter) {
    bindDiagnostics(crashReporter);
    refreshDiagnostics();
  }
  return { refreshSettings, refreshDiagnostics };
}
