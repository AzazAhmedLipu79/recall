// Channel<ProgressEvent> helpers — shared by every view that streams
// progress (import, verify, backup, export).
//
// Tauri 2 sometimes wraps the channel payload in `{ payload, ... }`
// and sometimes hands the raw event object back. Some Rust commands
// emit snake_case event types ("file_imported"), others emit
// camelCase ("fileImported"). `matchType` collapses both forms so the
// call site reads as `matchType(e, "file_imported")`.

export function unwrapEvent(ev) {
  return ev?.payload ?? ev;
}

// Match `e.type` against either snake_case or camelCase. Returns true
// if either form matches.
export function matchType(e, snake, camel) {
  const t = e?.type ?? "";
  if (t === snake) return true;
  if (camel && t === camel) return true;
  return false;
}