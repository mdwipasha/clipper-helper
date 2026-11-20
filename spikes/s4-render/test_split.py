import subprocess
import os

ffmpeg_exe = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")

# Test split layout:
# Top: ratio = 0.55 -> top_h = 1056, bottom_h = 864
top_h = 1056
bottom_h = 864

filtergraph = (
    f"[0:v]split=2[in_top][in_bottom];"
    f"[in_top]crop=w=iw:h=ih*0.5:x=0:y=0,scale=1080:{top_h}[top];"
    f"[in_bottom]crop=w=iw:h=ih*0.5:x=0:y=ih*0.5,scale=1080:{bottom_h}[bottom];"
    f"[top][bottom]vstack[v_out]"
)

cmd = [
    ffmpeg_exe, "-y",
    "-f", "lavfi", "-i", "testsrc=size=1920x1080:rate=30:duration=1",
    "-filter_complex", filtergraph,
    "-map", "[v_out]",
    "-c:v", "h264_mf", "-pix_fmt", "yuv420p",
    "-f", "null", "-"
]
res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
print("Split exit code:", res.returncode)
if res.returncode != 0:
    print("Stderr:", res.stderr[-500:])
else:
    print("Split filtergraph PASSED!")
