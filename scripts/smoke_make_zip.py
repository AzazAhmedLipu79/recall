#!/usr/bin/env python3
"""Synthesize a small Google Takeout-shaped ZIP for the smoke test."""
import json
import zipfile
from pathlib import Path

OUT = Path("/Users/darkmac/Desktop/recall/.smoke/takeout.zip")

# Three small "images" (just enough bytes to look like JPEGs).
img1 = bytes([0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xAA]) + b"vacation-one-payload"
img2 = bytes([0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xBB]) + b"vacation-two-payload"
img3 = bytes([0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xCC]) + b"plain-payload"

sidecar_v1 = {
    "title": "Vacation photo 1",
    "description": "Sunset at the beach",
    "photoTakenTime": {"timestamp": "1609459200"},
}
sidecar_v2 = {
    "title": "Vacation photo 2",
    "description": "Dinner by the water",
    "photoTakenTime": {"timestamp": "1609459300"},
}

OUT.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(OUT, "w", compression=zipfile.ZIP_STORED) as z:
    # Mimic Takeout's common top-level wrapper.
    # Two photos under "Albums/Vacation/" (should derive album = Vacation),
    # plus one photo under "Google Photos/Plain/" (no album from path).
    z.writestr("Takeout/Albums/Vacation/IMG_001.jpg", img1)
    z.writestr(
        "Takeout/Albums/Vacation/IMG_001.jpg.json",
        json.dumps(sidecar_v1),
    )
    z.writestr("Takeout/Albums/Vacation/IMG_002.jpg", img2)
    z.writestr(
        "Takeout/Albums/Vacation/IMG_002.jpg.json",
        json.dumps(sidecar_v2),
    )
    z.writestr("Takeout/Google Photos/Plain/IMG_003.jpg", img3)

print(f"Wrote {OUT} ({OUT.stat().st_size} bytes)")
