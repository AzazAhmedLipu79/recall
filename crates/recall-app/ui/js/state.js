// Shared UI state. Only values genuinely used across multiple views
// live here — everything else is owned by its view module.
//
// Exposed as `let` bindings behind named setter functions so changes
// are greppable and views can't accidentally share references.

// --- Sidebar filter state (library + albums list) ---

export let currentKind = "";
export function setCurrentKind(v) { currentKind = v; }

export let currentAlbum = "";
export function setCurrentAlbum(v) { currentAlbum = v; }

// --- Inspector / lightbox selection ---

export let selectedId = null;
export function setSelectedId(v) { selectedId = v; }
export function getSelectedId() { return selectedId; }

// --- Import view mode ("takeout" | "folder") ---

export let importMode = "takeout";
export function setImportMode(v) { importMode = v; }

// --- Most recent search results (used by lightbox for prev/next) ---

export let currentResults = [];
export function setCurrentResults(v) { currentResults = v ?? []; }