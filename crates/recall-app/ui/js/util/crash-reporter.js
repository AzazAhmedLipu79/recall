// Opt-in local crash reporting.
//
// When the user toggles "Log JS errors locally" ON in Settings, this
// module installs `window.onerror` + `unhandledrejection` listeners
// that append a structured entry to `<archive>/logs/js-errors.log`
// via the `cmd_log_js_error` Tauri command. Nothing leaves the
// machine — the user attaches the log to a bug report themselves.
//
// The toggle is read on init and on every change, so flipping the
// switch in Settings takes effect immediately for the running session.

import { getSetting, setSetting } from "./store.js";
import { cmdLogJsError, cmdRevealInFinder, cmdLogDir } from "../api.js";
import { toast } from "./toast.js";

const KEY = "recall_js_error_logging";

async function reportError(message, stack, url) {
  try {
    // The backend caps the per-entry size implicitly via the open +
    // append loop. Keep the message short for readability.
    const m = String(message ?? "").slice(0, 2000);
    const s = stack ? String(stack).slice(0, 8000) : null;
    const u = url ? String(url).slice(0, 500) : null;
    await cmdLogJsError(m, s, u);
  } catch (e) {
    // Last-resort: console only. We must never throw from inside an
    // error handler — that would mask the original failure.
    console.warn("cmd_log_js_error failed:", e);
  }
}

function installHandlers() {
  window.addEventListener("error", (e) => {
    reportError(e.message ?? "(no message)", e.error?.stack ?? null, e.filename ?? null);
  });
  window.addEventListener("unhandledrejection", (e) => {
    const reason = e.reason;
    const msg = reason instanceof Error ? reason.message : String(reason);
    const stack = reason instanceof Error ? reason.stack : null;
    reportError(`Unhandled rejection: ${msg}`, stack, null);
  });
}

export function initCrashReporter() {
  let enabled = false;
  getSetting(KEY, false).then((v) => { enabled = Boolean(v); });
  installHandlers();

  return {
    async setEnabled(on) {
      enabled = Boolean(on);
      await setSetting(KEY, enabled);
    },
    isEnabled: () => enabled,
    async revealLogsFolder() {
      try {
        const dir = await cmdLogDir();
        await cmdRevealInFinder(dir);
      } catch (e) {
        toast("Couldn't reveal logs folder", String(e), "err");
      }
    },
  };
}