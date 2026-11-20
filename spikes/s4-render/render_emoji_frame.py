import subprocess
import os

ffmpeg_exe = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")
ass_path = os.path.abspath("spikes/s4-render/results/caption_emoji_test.ass")
fonts_dir = os.path.abspath("spikes/s4-render/fonts")
out_frame = os.path.abspath("spikes/s4-render/results/emoji_frame.png")

posix_ass = ass_path.replace("\\", "/").replace(":", r"\:")
posix_fonts = fonts_dir.replace("\\", "/").replace(":", r"\:")
vf = f"ass=filename='{posix_ass}':fontsdir='{posix_fonts}'"

cmd = [
    ffmpeg_exe, "-y",
    "-f", "lavfi", "-i", "color=c=black:s=1080x1920:d=1:r=30",
    "-vf", vf,
    "-frames:v", "1",
    out_frame
]
res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
print("Render emoji PNG exit code:", res.returncode)
print("PNG size:", os.path.getsize(out_frame) if os.path.exists(out_frame) else 0)
