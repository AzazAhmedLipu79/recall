// Wrapper around tauri-plugin-dialog so views don't need to know the
// plugin's location. Tauri 2 with `withGlobalTauri: true` exposes
// the plugin at `window.__TAURI__.dialog`.
//
// Native prompt/confirm fall back to window.prompt/confirm in case
// the runtime plugin call fails (e.g. web preview, missing permission).

function plugin() { return window.__TAURI__?.dialog; }

export async function pickFolder({ title = "Choose folder", defaultPath } = {}) {
  const dlg = plugin();
  if (!dlg?.open) throw new Error("tauri-plugin-dialog not available");
  return await dlg.open({ directory: true, multiple: false, title, defaultPath });
}

export async function askText({ title, message, defaultValue = "" } = {}) {
  const dlg = plugin();
  if (dlg?.ask) {
    const v = await dlg.ask(message, { title, defaultValue });
    return v ?? null;
  }
  return window.prompt(message ?? title ?? "", defaultValue);
}

export async function confirmAction({ title, message, okLabel = "OK", cancelLabel = "Cancel" } = {}) {
  const dlg = plugin();
  if (dlg?.confirm) {
    return Boolean(await dlg.confirm(message, { title, okLabel, cancelLabel }));
  }
  return window.confirm(message ?? title ?? "");
}
