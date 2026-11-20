import os
import urllib.request
import shutil

FONTS_DIR = os.path.join(os.path.dirname(__file__), "fonts")
os.makedirs(FONTS_DIR, exist_ok=True)

# Copy Montserrat if present in s5-ffmpeg
s5_montserrat = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "s5-ffmpeg", "fonts", "Montserrat-VariableFont_wght.ttf"))
target_montserrat = os.path.join(FONTS_DIR, "Montserrat-VariableFont_wght.ttf")
if os.path.exists(s5_montserrat) and not os.path.exists(target_montserrat):
    shutil.copyfile(s5_montserrat, target_montserrat)
    print("Copied Montserrat from s5-ffmpeg")

# Fonts URLs from Google Fonts official repository
FONT_URLS = {
    "Montserrat-Bold.ttf": "https://raw.githubusercontent.com/google/fonts/main/ofl/montserrat/Montserrat%5Bwght%5D.ttf",
    "Poppins-Bold.ttf": "https://raw.githubusercontent.com/google/fonts/main/ofl/poppins/Poppins-Bold.ttf",
    "Inter-Bold.ttf": "https://raw.githubusercontent.com/google/fonts/main/ofl/inter/Inter%5Bopsz%2Cwght%5D.ttf",
    "Anton-Regular.ttf": "https://raw.githubusercontent.com/google/fonts/main/ofl/anton/Anton-Regular.ttf",
}

for name, url in FONT_URLS.items():
    dest = os.path.join(FONTS_DIR, name)
    if not os.path.exists(dest):
        print(f"Downloading {name} from {url}...")
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
            with urllib.request.urlopen(req) as resp, open(dest, "wb") as out:
                out.write(resp.read())
            print(f"Downloaded {name} ({os.path.getsize(dest)} bytes)")
        except Exception as e:
            print(f"Failed to download {name}: {e}")
    else:
        print(f"{name} already exists ({os.path.getsize(dest)} bytes)")

print("Fonts setup completed.")
