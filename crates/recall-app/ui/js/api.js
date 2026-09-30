// Typed-ish wrappers for every Tauri command we call from the UI.
// Importing this module also captures `__TAURI__` globals once, on
// module evaluation, so any `withGlobalTauri` race is settled before
// the first view tries to invoke.
//
// We re-export `invoke`, `Channel`, `convertFileSrc`, and
// `getCurrentWebview` for modules that need them directly.

const TAURI = window.__TAURI__ || null;

if (!TAURI) {
  // Don't throw — modules that don't need IPC (e.g. dom.js) should
  // still load. We let callers surface "Tauri runtime not available"
  // via `assertTauri()` in main.js.
  // eslint-disable-next-line no-console
  console.warn("window.__TAURI__ missing at module load — IPC will throw.");
}

export const invoke = TAURI?.core?.invoke
  ?? (async () => { throw new Error("Tauri runtime not available"); });

export const Channel = TAURI?.core?.Channel;

export const convertFileSrc = TAURI?.core?.convertFileSrc
  ?? ((p) => `asset://localhost/${encodeURI(p)}`);

export const getCurrentWebview = TAURI?.webview?.getCurrentWebview
  ?? TAURI?.webviewWindow?.getCurrentWebview;

// --- Domain enums (mirror Rust types in commands.rs) ---
export const ALBUM_KIND = Object.freeze({
  User: "User",
  GoogleTakeout: "GoogleTakeout",
});

// --- Domain wrappers ---

// Status / counts
export const cmdStatus        = ()        => invoke("cmd_status");
export const cmdArchiveInfo   = ()        => invoke("cmd_archive_info");
export const cmdListImports   = (limit=10)=> invoke("cmd_list_imports", { limit });

// Library search
export const cmdSearch        = (args)    => invoke("cmd_search", { args });

// Media detail (for inspector)
export const cmdGetMediaDetail = (id)     => invoke("cmd_get_media_detail", { id });

// Albums
export const cmdListAlbums          = () => invoke("cmd_list_albums");
export const cmdCreateAlbum         = (name) => invoke("cmd_create_album", { name });
export const cmdRenameAlbum         = (id, name) => invoke("cmd_rename_album", { id, name });
export const cmdDeleteAlbum         = (id) => invoke("cmd_delete_album", { id });
export const cmdAddToAlbum          = (albumId, mediaId) => invoke("cmd_add_to_album", { albumId, mediaId });
export const cmdRemoveFromAlbum     = (albumId, mediaId) => invoke("cmd_remove_from_album", { albumId, mediaId });
export const cmdGetAlbumsForMedia   = (mediaId) => invoke("cmd_get_albums_for_media", { mediaId });

// Imports
export const cmdImportFolder  = (args) => invoke("cmd_import_folder",  args);
export const cmdImportTakeout = (args) => invoke("cmd_import_takeout", args);

// Maintenance
export const cmdVerify         = (channel) => invoke("cmd_verify",        { channel });
export const cmdBackup         = (args)     => invoke("cmd_backup",        args);
export const cmdExportZip      = (args)     => invoke("cmd_export_zip",    args);

// Wizard / first-run
export const cmdIsFirstRun     = () => invoke("cmd_is_first_run");
export const cmdSeedSample     = (count = 20) => invoke("cmd_seed_sample", { count });

// OS helpers
export const cmdRevealInFinder = (path) => invoke("cmd_reveal_in_finder", { path });
export const cmdLogDir         = () => invoke("cmd_log_dir");
export const cmdLogJsError     = (message, stack, url) => invoke("cmd_log_js_error", { message, stack, url });

// Archive location
export const cmdSetArchiveRoot = (newRoot) => invoke("cmd_set_archive_root", { newRoot });
export const cmdMigrateArchive = (args) => invoke("cmd_migrate_archive", args);
export const cmdProbeArchiveRoot = (path) => invoke("cmd_probe_archive_root", { path });