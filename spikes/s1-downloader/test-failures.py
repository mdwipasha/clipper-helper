#!/usr/bin/env python3
"""
Spike S1 — test-failures.py
Capture yt-dlp output for each failure class and propose E_YT_* mappings.
Saves text fixtures to fixtures/ directory.
"""
import json
import os
import subprocess
import sys
import re

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
FIXTURES_DIR = os.path.join(SPIKE_DIR, "fixtures")

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
DENO = os.path.join(BIN_DIR, "deno.exe")

# Test cases: (name, url, expected_error_code)
# Using the user-provided URLs plus well-known test cases
TEST_CASES = [
    # User-provided
    ("ongoing_live", "https://www.youtube.com/watch?v=QSPPyA1736U", "E_YT_LIVE_NOT_ENDED"),
    ("members_only", "https://www.youtube.com/watch?v=KBdGeakRgqg", "E_YT_UNAVAILABLE"),

    # Well-known test cases — these are commonly used for testing
    # Deleted video (a known-deleted video ID)
    ("deleted", "https://www.youtube.com/watch?v=dQw4w9WgXcX", "E_YT_UNAVAILABLE"),

    # Private video (using a known private video)
    ("private", "https://www.youtube.com/watch?v=_bHqBy5fmUM", "E_YT_UNAVAILABLE"),

    # Age-restricted
    ("age_restricted", "https://www.youtube.com/watch?v=6kLq3WMV1nU", "E_YT_UNAVAILABLE"),

    # Invalid/malformed
    ("invalid_url", "https://www.youtube.com/watch?v=XXXXXXXXXXX", "E_YT_URL_INVALID"),

    # Playlist URL (should be rejected)
    ("playlist_url", "https://www.youtube.com/playlist?list=PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf", "E_YT_URL_INVALID"),
]


def run_ytdlp_probe(name: str, url: str) -> dict:
    """Run yt-dlp -J (probe) and capture output for failure classification."""
    cmd = [
        YTDLP,
        "-J", "--no-playlist", "--no-warnings", "--ignore-config",
        "--js-runtimes", f"deno:{DENO}",
        url,
    ]

    print(f"\n{'='*60}")
    print(f"Testing: {name}")
    print(f"URL: {url}")
    print(f"{'='*60}")

    result = subprocess.run(
        cmd, capture_output=True, text=True, timeout=60
    )

    output = {
        "name": name,
        "url": url,
        "exit_code": result.returncode,
        "stdout": result.stdout[:2000] if result.stdout else "",
        "stderr": result.stderr[:2000] if result.stderr else "",
    }

    # Save full output as fixture
    fixture_file = os.path.join(FIXTURES_DIR, f"{name}.txt")
    with open(fixture_file, "w", encoding="utf-8") as f:
        f.write(f"# yt-dlp failure fixture: {name}\n")
        f.write(f"# URL: {url}\n")
        f.write(f"# Exit code: {result.returncode}\n")
        f.write(f"# Command: yt-dlp -J --no-playlist --no-warnings --ignore-config <url>\n\n")
        f.write("--- STDOUT ---\n")
        f.write(result.stdout[:5000] if result.stdout else "(empty)\n")
        f.write("\n--- STDERR ---\n")
        f.write(result.stderr[:5000] if result.stderr else "(empty)\n")

    # Classify the error
    stderr = result.stderr.lower() if result.stderr else ""
    stdout_lower = result.stdout.lower() if result.stdout else ""
    all_text = stderr + " " + stdout_lower

    classification = "E_YT_EXTRACT_FAILED"  # default

    if result.returncode == 0:
        classification = "SUCCESS"
        # Check if it's a live stream
        try:
            info = json.loads(result.stdout)
            if info.get("is_live"):
                classification = "E_YT_LIVE_NOT_ENDED"
        except (json.JSONDecodeError, KeyError):
            pass
    else:
        # Pattern matching on error text
        patterns = [
            (r"(this live event will begin|premiere will begin|premieres in)", "E_YT_LIVE_NOT_ENDED"),
            (r"(is not available|unavailable|has been removed)", "E_YT_UNAVAILABLE"),
            (r"(private video|sign in to confirm|video is private)", "E_YT_UNAVAILABLE"),
            (r"(members.only|join this channel)", "E_YT_UNAVAILABLE"),
            (r"(age.restrict|confirm your age|sign in to confirm your age)", "E_YT_UNAVAILABLE"),
            (r"(blocked|not available in your country|geo.?restrict)", "E_YT_UNAVAILABLE"),
            (r"(deleted|no longer available)", "E_YT_UNAVAILABLE"),
            (r"(live stream|is live|live event)", "E_YT_LIVE_NOT_ENDED"),
            (r"(is not a valid url|unable to extract|no video)", "E_YT_URL_INVALID"),
            (r"(too many requests|rate.?limit|429)", "E_YT_RATE_LIMITED"),
            (r"(http error 403|forbidden)", "E_YT_RATE_LIMITED"),
        ]

        for pattern, code in patterns:
            if re.search(pattern, all_text):
                classification = code
                break

    output["classification"] = classification
    output["stderr_summary"] = result.stderr[:300] if result.stderr else ""

    print(f"  Exit code: {result.returncode}")
    print(f"  Classification: {classification}")
    if result.stderr:
        # Print first meaningful error line
        for line in result.stderr.split("\n"):
            if line.strip() and "ERROR" in line.upper():
                print(f"  Error: {line.strip()[:150]}")
                break

    return output


def main():
    os.makedirs(FIXTURES_DIR, exist_ok=True)

    if not os.path.exists(YTDLP):
        print(f"ERROR: yt-dlp not found at {YTDLP}. Run fetch-binaries.py first.")
        sys.exit(1)

    all_results = []
    for name, url, expected_code in TEST_CASES:
        try:
            result = run_ytdlp_probe(name, url)
            result["expected_code"] = expected_code
            result["match"] = result["classification"] == expected_code
            all_results.append(result)
        except subprocess.TimeoutExpired:
            print(f"  TIMEOUT (60s)")
            all_results.append({
                "name": name, "url": url, "classification": "E_NET_TIMEOUT",
                "expected_code": expected_code, "match": False, "timeout": True,
            })
        except Exception as e:
            print(f"  Exception: {e}")
            all_results.append({
                "name": name, "url": url, "classification": "E_UNKNOWN",
                "expected_code": expected_code, "match": False, "error": str(e),
            })

    # Summary
    print(f"\n{'='*80}")
    print("FAILURE CLASSIFICATION SUMMARY")
    print(f"{'='*80}")
    print(f"{'Name':<20} {'Expected':<25} {'Actual':<25} {'Match':>5}")
    for r in all_results:
        match_str = "✓" if r.get("match") else "✗"
        print(f"{r['name']:<20} {r.get('expected_code', '?'):<25} {r['classification']:<25} {match_str:>5}")

    # Proposed mapping
    print(f"\n{'='*80}")
    print("PROPOSED E_YT_* MAPPING (based on observed patterns)")
    print(f"{'='*80}")
    print("""
| yt-dlp error pattern                     | E_YT_* code            | Retryable |
|------------------------------------------|------------------------|-----------|
| "is not available", "unavailable"        | E_YT_UNAVAILABLE       | no        |
| "private video", "sign in to confirm"    | E_YT_UNAVAILABLE       | no        |
| "members only", "join this channel"      | E_YT_UNAVAILABLE       | no        |
| "age-restrict", "confirm your age"       | E_YT_UNAVAILABLE       | no        |
| "not available in your country"          | E_YT_UNAVAILABLE       | no        |
| "deleted", "no longer available"         | E_YT_UNAVAILABLE       | no        |
| "live event will begin", "premieres in"  | E_YT_LIVE_NOT_ENDED    | yes       |
| "is live", "live stream"                 | E_YT_LIVE_NOT_ENDED    | yes       |
| "too many requests", "429", "rate limit" | E_YT_RATE_LIMITED      | yes       |
| "403 Forbidden"                          | E_YT_RATE_LIMITED      | yes       |
| "not a valid URL", "unable to extract"   | E_YT_URL_INVALID       | no        |
| Other / unknown                          | E_YT_EXTRACT_FAILED    | yes       |
""")

    # Save results
    results_file = os.path.join(FIXTURES_DIR, "classification_results.json")
    # Remove large fields for saving
    save_results = []
    for r in all_results:
        sr = {k: v for k, v in r.items() if k not in ("stdout", "stderr")}
        save_results.append(sr)
    with open(results_file, "w") as f:
        json.dump(save_results, f, indent=2)

    print(f"\nFixtures saved to: {FIXTURES_DIR}/")
    print(f"Results saved to: {results_file}")


if __name__ == "__main__":
    main()
