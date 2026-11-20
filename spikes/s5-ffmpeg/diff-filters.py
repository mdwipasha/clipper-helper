import subprocess
import re

def get_filters(exe):
    out = subprocess.check_output([exe, "-filters"], text=True, errors="ignore")
    filters = {}
    for line in out.splitlines():
        m = re.match(r"^\s*([TSC.]{2,3})\s+([a-zA-Z0-9_]+)\s+([A-Z|N>-]+)\s+(.*)$", line)
        if m:
            filters[m.group(2)] = (m.group(1), m.group(3), m.group(4))
    return filters

gpl_filters = get_filters("spikes/s1-downloader/bin/ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe")
lgpl_filters = get_filters("spikes/s5-ffmpeg/bin/ffmpeg-release-shared/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0/bin/ffmpeg.exe")

diff = sorted(list(set(gpl_filters.keys()) - set(lgpl_filters.keys())))

lines = [
    "# GPL-Only FFmpeg Filters Denylist",
    "# Generated: 2026-11-20 via Spike S5 follow-up",
    "# Reference build comparison:",
    "#   GPL: ffmpeg 9.0.2-essentials_build-www.gyan.dev (--enable-gpl)",
    "#   LGPL: ffmpeg n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0 (--disable-gpl, --enable-version3)",
    "#",
    "# RULE (AGENTS.md §5, ROADMAP §4):",
    "# Render filters used in Clipper Helper MUST NOT appear in this denylist.",
    "# All composition filters must exist in the pinned LGPL build capability JSON.",
    "# CI golden render snapshot tests assert absence of these filters.",
    f"# Total GPL-only filters identified: {len(diff)}",
    "# -----------------------------------------------------------------------------",
    "# Filter Name            Flags  Direction  Description",
    "# -----------------------------------------------------------------------------"
]

for f in diff:
    flags, io_dir, desc = gpl_filters[f]
    lines.append(f"{f:24} {flags:6} {io_dir:10} {desc}")

with open("docs/benchmarks/gpl-filters-denylist.txt", "w", encoding="utf-8") as fp:
    fp.write("\n".join(lines) + "\n")

print(f"Wrote docs/benchmarks/gpl-filters-denylist.txt successfully with {len(diff)} filters.")
