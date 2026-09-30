// mediaUrl / getMediaPath — resolve a media id to an `asset://` URL
// the webview can stream.
//
// We ask the backend (cmd_get_media_path) for the absolute path once,
// cache it for the lifetime of the page, and then hand the URL to
// <img>/<video>/<audio>. If the archive moves (a very rare event),
// a refresh repopulates the cache.
//
// `invoke` is the Tauri command runner passed in by the caller so this
// module stays unit-testable (and so we don't grab window.__TAURI__ at
// import time, which can race with withGlobalTauri).

const mediaPathCache = new Map(); // id -> absolute path (or null)

export async function getMediaPath(id, invoke) {
  if (mediaPathCache.has(id)) return mediaPathCache.get(id);
  try {
    const path = await invoke("cmd_get_media_path", { id });
    mediaPathCache.set(id, path ?? null);
    return path ?? null;
  } catch (e) {
    console.error("cmd_get_media_path failed for", id, e);
    mediaPathCache.set(id, null);
    return null;
  }
}

// `convertFileSrc` is Tauri.core.convertFileSrc — wraps an absolute path
// into an asset:// URL the webview can stream.
export async function mediaUrl(id, invoke, convertFileSrc) {
  const path = await getMediaPath(id, invoke);
  if (!path) return null;
  return convertFileSrc(path);
}

// Clear the path cache. Used when the archive is moved or the user
// explicitly refreshes.
export function clearMediaPathCache() {
  mediaPathCache.clear();
}