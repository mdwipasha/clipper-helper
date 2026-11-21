import zipfile
import os

zip_path = "spikes/s3-speech/bin/whisper-blas-bin-x64.zip"
dest_dir = "spikes/s3-speech/bin/whisper-blas-bin-x64"

if os.path.exists(zip_path) and not os.path.exists(dest_dir):
    print(f"Extracting {zip_path} -> {dest_dir}...")
    with zipfile.ZipFile(zip_path, "r") as zf:
        zf.extractall(dest_dir)
    print("Done extracting OpenBLAS build.")
else:
    print("Already extracted or zip missing.")
