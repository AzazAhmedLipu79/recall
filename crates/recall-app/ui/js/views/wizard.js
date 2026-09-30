// First-run wizard — four steps:
//
//   1. Welcome      — product pitch + "Get started →"
//   2. Archive path — explain what the archive is, show the resolved
//                     path, "Change…" opens the native folder picker
//                     and applies it via cmd_set_archive_root (no
//                     restart needed).
//   3. Sample seed  — "Try with 20 sample photos" runs cmd_seed_sample(20).
//   4. All set      — recap the chosen path, route the user to the library.
//
// Skippable from steps 1 + 2. Skip / Done sets `recall_wizard_seen` in
// tauri-plugin-store so the wizard doesn't reappear on next launch.

import { $ } from "../util/dom.js";
import { toast } from "../util/toast.js";
import { pickFolder } from "../util/dialog.js";
import { getSetting, setSetting } from "../util/store.js";
import { trapFocus } from "../util/focus-trap.js";
import { cmdArchiveInfo, cmdIsFirstRun, cmdSeedSample, cmdSetArchiveRoot } from "../api.js";

const WIZARD_SEEN_KEY = "recall_wizard_seen";
const STEPS = ["1", "2", "3", "4"];

function wizEl(step) { return $(`wiz-${step}`); }

function showStep(step) {
  STEPS.forEach((n) => {
    const el = wizEl(n);
    if (el) el.classList.toggle("hidden", n !== step);
  });
}

let _releaseTrap = null;
function hideWizard() {
  $("wizard")?.classList.add("hidden");
  if (_releaseTrap) { _releaseTrap(); _releaseTrap = null; }
}
function showWizard() {
  $("wizard")?.classList.remove("hidden");
  showStep("1");
  if (_releaseTrap) _releaseTrap();
  _releaseTrap = trapFocus($("wizard"), { onClose: finishWizard });
}

let _onFinish = () => {};
let _runSearch = () => {};
let _refreshStatus = () => {};
// Updated by `cmdSetArchiveRoot`; read by step 4's "Open the library"
// recap so we don't have to fire a second `cmdArchiveInfo` round-trip.
let _archivePath = "(unknown)";

async function resolveArchivePath() {
  try {
    const info = await cmdArchiveInfo();
    _archivePath = info.archive_root;
  } catch (e) {
    console.warn("cmd_archive_info failed in wizard:", e);
  }
  return _archivePath;
}

function bindStep1() {
  wizEl("1-next")?.addEventListener("click", async () => {
    const path = await resolveArchivePath();
    $("wiz-archive-path").textContent = path;
    showStep("2");
  });
}

function bindStep2() {
  wizEl("2-change")?.addEventListener("click", async () => {
    try {
      const picked = await pickFolder({
        title: "Choose your Recall archive folder",
        defaultPath: _archivePath,
      });
      if (!picked) return;
      const btn = wizEl("2-change");
      const original = btn?.textContent;
      if (btn) { btn.disabled = true; btn.textContent = "Applying…"; }
      try {
        // In-place switch — no restart required.
        await cmdSetArchiveRoot(picked);
        _archivePath = picked;
        $("wiz-archive-path").textContent = picked;
        toast("Archive location set", picked, "ok", 2500);
      } catch (e) {
        toast("Couldn't change archive location", String(e), "err");
      } finally {
        if (btn) { btn.disabled = false; btn.textContent = original ?? "Change…"; }
      }
    } catch (e) {
      toast("Couldn't open folder picker", String(e), "err");
    }
  });
  wizEl("2-next")?.addEventListener("click", () => showStep("3"));
}

function bindStep3() {
  wizEl("3-skip")?.addEventListener("click", () => showStep("4"));
  wizEl("3-next")?.addEventListener("click", async () => {
    // Render step 4 with the resolved path before advancing.
    $("wiz-4-path").textContent = _archivePath ?? "(unknown)";
    showStep("4");
  });
  wizEl("3-seed")?.addEventListener("click", async (e) => {
    const seed = e.currentTarget;
    seed.disabled = true;
    const original = seed.textContent;
    seed.textContent = "Seeding…";
    try {
      await cmdSeedSample(20);
      toast("Sample seed complete", "20 sample photos imported.", "ok");
    } catch (err) {
      toast("Sample seed failed", String(err), "err");
    } finally {
      seed.disabled = false;
      seed.textContent = original;
    }
  });
}

function bindStep4() {
  wizEl("4-done")?.addEventListener("click", async () => {
    await finishWizard();
    _refreshStatus();
    _runSearch();
  });
}

// "Skip setup" links on steps 1 + 2 are bare <button data-action="wiz-skip">;
// delegated once at init so re-init can't double-bind.
function bindSkipLinks() {
  document.addEventListener("click", (e) => {
    if (e.target.closest('[data-action="wiz-skip"]')) finishWizard();
  });
}

async function finishWizard() {
  hideWizard();
  await setSetting(WIZARD_SEEN_KEY, true);
  _onFinish();
}

async function checkFirstRun() {
  let isFirst;
  try {
    isFirst = await cmdIsFirstRun();
  } catch (e) {
    console.warn("cmd_is_first_run unavailable:", e);
    return;
  }
  if (!isFirst) return;
  // Honour the wizard-seen flag so we don't pester returning users
  // whose archive happens to be empty due to a rollback, wipe, etc.
  if (await getSetting(WIZARD_SEEN_KEY, false)) return;
  showWizard();
}

export function initWizard({ onFinish, runSearch, refreshStatus }) {
  if (typeof onFinish !== "function") throw new Error("initWizard: onFinish is required");
  if (typeof runSearch !== "function") throw new Error("initWizard: runSearch is required");
  if (typeof refreshStatus !== "function") throw new Error("initWizard: refreshStatus is required");
  _onFinish = onFinish;
  _runSearch = runSearch;
  _refreshStatus = refreshStatus;
  bindStep1();
  bindStep2();
  bindStep3();
  bindStep4();
  bindSkipLinks();
  return { checkFirstRun, showWizard, hideWizard };
}
