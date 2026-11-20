import subprocess
import os

ffmpeg_exe = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")

# Test blur_fit filtergraph:
# Input: 1920x1080 horizontal test pattern
# Output: 1080x1920 vertical video
# Background: scale to fill 1080x1920, blur using gblur
# Foreground: scale to fit 1080x1920 (contain)
# Overlay: center foreground on blurred background
filtergraph = (
    "[0:v]split=2[bg_in][fg_in];"
    "[bg_in]scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,gblur=sigma=30:steps=2[bg];"
    "[fg_in]scale=1080:1920:force_original_aspect_ratio=decrease[fg];"
    "[bg][fg]overlay=(W-w)/2:(H-h)/2[v_out]"
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
print("Blur_fit exit code:", res.returncode)
if res.returncode != 0:
    print("Stderr:", res.stderr[-500:])
else:
    print("Blur_fit filtergraph PASSED!")
