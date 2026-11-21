import urllib.request
import json
import ssl

ctx = ssl.create_default_context()
req = urllib.request.Request(
    'https://huggingface.co/api/models/ggml-org/whisper-vad/tree/main',
    headers={'User-Agent': 'Mozilla/5.0'}
)
try:
    with urllib.request.urlopen(req, context=ctx) as resp:
        files = json.loads(resp.read().decode('utf-8'))
        for f in files:
            p = f.get('path', '')
            print("Found VAD file:", p, f.get('size'), "bytes")
except Exception as e:
    print('Error:', e)
