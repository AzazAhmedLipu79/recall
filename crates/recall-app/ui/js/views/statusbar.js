// Status bar — counts (photos/videos/docs/audio) + archive root path
// as a clickable badge that reveals the folder in Finder.

import { $ } from "../util/dom.js";
import { cmdStatus, cmdArchiveInfo, cmdRevealInFinder } from "../api.js";
import { fmtSize } from "../util/format.js";
import { toast } from "../util/toast.js";

async function refreshStatus() {
  // cmdStatus and cmdArchiveInfo hit independent commands and write to
  // disjoint DOM regions; safe to parallelise.
  const [countsResult, infoResult] = await Promise.allSettled([
    cmdStatus(),
    cmdArchiveInfo(),
  ]);
  if (countsResult.status === "fulfilled") {
    const counts = countsResult.value;
    $("status-summary").textContent =
      `${counts.photos} photos · ${counts.videos} videos · ${counts.documents} docs · ${counts.audio} audio`;
    const total = (counts.photos || 0) + (counts.videos || 0) + (counts.documents || 0) + (counts.audio || 0) + (counts.other || 0);
    $("cnt-all").textContent = total;
    $("cnt-photo").textContent = counts.photos ?? 0;
    $("cnt-video").textContent = counts.videos ?? 0;
    $("cnt-audio").textContent = counts.audio ?? 0;
    $("cnt-document").textContent = counts.documents ?? 0;
  } else {
    $("status-summary").textContent = "status unavailable";
  }
  if (infoResult.status === "fulfilled") {
    renderArchiveBadge(infoResult.value);
  }
}

function renderArchiveBadge(info) {
  const pathEl = $("status-archive-path");
  const sizeEl = $("status-archive-size");
  const badge  = $("status-archive");
  if (pathEl) pathEl.textContent = info.archive_root;
  if (sizeEl) sizeEl.textContent = info.disk_usage_bytes > 0
    ? ` · ${fmtSize(info.disk_usage_bytes)}`
    : "";
  if (badge) {
    badge.title = info.is_custom
      ? `Click to reveal in Finder\n${info.archive_root}`
      : `Click to reveal in Finder (default location)\n${info.archive_root}`;
    badge.setAttribute("aria-label", `Archive at ${info.archive_root}. Click to show in Finder.`);
  }
}

function bindArchiveBadge() {
  const badge = $("status-archive");
  if (!badge) return;
  badge.addEventListener("click", async () => {
    try {
      const info = await cmdArchiveInfo();
      await cmdRevealInFinder(info.archive_root);
    } catch (e) {
      toast("Couldn't reveal archive", String(e), "err");
    }
  });
}

export function initStatusbar() {
  bindArchiveBadge();
  return { refreshStatus };
}