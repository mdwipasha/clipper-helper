import subprocess
import os

ffmpeg_exe = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")
ass_file = os.path.abspath("spikes/s5-ffmpeg/results/test_caption.ass")
fonts_dir = os.path.abspath("spikes/s4-render/fonts")

def escape_filter_path(p):
    p_posix = p.replace("\\", "/")
    return p_posix.replace(":", r"\:")

ass_esc = escape_filter_path(ass_file)
fonts_esc = escape_filter_path(fonts_dir)

filter_str = f"ass=filename='{ass_esc}':fontsdir='{fonts_esc}'"
print("Filter string:", filter_str)

cmd = [
    ffmpeg_exe, "-y",
    "-f", "lavfi", "-i", "color=c=black:s=1080x1920:d=1:r=30",
    "-vf", filter_str,
    "-c:v", "h264_mf", "-pix_fmt", "yuv420p",
    "-f", "null", "-"
]
res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
print("Exit code:", res.returncode)
if res.returncode != 0:
    print("Stderr:", res.stderr[-500:])
else:
    print("Filter test PASSED!")
