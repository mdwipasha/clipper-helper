import json

with open("spikes/s3-speech/results/test_run/podcast_dtw.json", "r", encoding="utf-8") as f:
    data = json.load(f)

for i, seg in enumerate(data.get("transcription", [])[:3]):
    print(f"\n--- Segment {i} [{seg['timestamps']['from']} -> {seg['timestamps']['to']}] (offsets: {seg['offsets']}) ---")
    print(f"Text: '{seg['text']}'")
    print("Tokens:")
    for tok in seg.get("tokens", []):
        print(f"  id={tok['id']:5} text={tok['text']!r:15} offsets={tok['offsets']} p={tok['p']:.3f} t_dtw={tok.get('t_dtw')}")
