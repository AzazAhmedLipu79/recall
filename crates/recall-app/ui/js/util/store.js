// Wrapper around tauri-plugin-store for the small set of keys we
// persist in v1. The store file lives at
// `<app_config>/settings.json` (the plugin's default location).
//
// Tauri 2 with `withGlobalTauri: true` exposes the plugin as
// `window.__TAURI__.store` — we lazy-load that once on first use so
// views that never touch persisted settings don't pay the cost.

const STORE_FILE = "settings.json";
let _storePromise = null;

async function getStore() {
  if (_storePromise) return _storePromise;
  _storePromise = (async () => {
    const plugin = window.__TAURI__?.store;
    if (!plugin || typeof plugin.load !== "function") {
      throw new Error("tauri-plugin-store not available (window.__TAURI__.store)");
    }
    return await plugin.load(STORE_FILE, { autoSave: true });
  })().catch((e) => {
    // Reset so a later call can retry after the runtime arrives.
    _storePromise = null;
    throw e;
  });
  return _storePromise;
}

export async function getSetting(key, fallback = null) {
  try {
    const store = await getStore();
    const v = await store.get(key);
    return v ?? fallback;
  } catch (e) {
    console.warn(`store.get(${key}) failed:`, e);
    return fallback;
  }
}

export async function setSetting(key, value) {
  try {
    const store = await getStore();
    await store.set(key, value);
  } catch (e) {
    console.warn(`store.set(${key}) failed:`, e);
  }
}
