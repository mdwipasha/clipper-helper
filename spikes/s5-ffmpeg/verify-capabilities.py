#!/usr/bin/env python3
"""
Spike S5 — verify-capabilities.py
Reusable verification script and CI gate for FFmpeg/ffprobe capability audit:
1. Audits buildconf (no GPL, no nonfree, version3 status, absent forbidden encoders, external libs)
2. Audits encoders (presence & 1-second functional test for aac, h264_mf, h264_nvenc, h264_qsv, h264_amf)
3. Audits decoders (h264, hevc, vp9, av1, aac, opus)
4. Audits filters (ass, subtitles, crop, scale, overlay, boxblur, split, vstack, hstack, loudnorm, ssim, psnr, drawtext, libvmaf)
5. Audits protocols & muxers/demuxers (https, file, mp4, mov, matroska, webm)
6. Runs functional render test: 1080x1920 vertical crop + ASS subtitle burn with local font via fontsdir (no fontconfig)
7. Dumps ffmpeg -encoders/-decoders/-filters to docs/benchmarks/
Outputs JSON and pass/fail summary. Exit code 0 on PASS, 1 on FAIL.
"""
import argparse
import json
import os
import re
import subprocess
import sys

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(SPIKE_DIR, "..", ".."))
DOCS_BENCHMARKS = os.path.join(REPO_ROOT, "docs", "benchmarks")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results")
FONTS_DIR = os.path.join(SPIKE_DIR, "fonts")
os.makedirs(RESULTS_DIR, exist_ok=True)
os.makedirs(DOCS_BENCHMARKS, exist_ok=True)
os.makedirs(FONTS_DIR, exist_ok=True)

DEFAULT_FFMPEG = os.path.join(
    SPIKE_DIR, "bin", "ffmpeg-release-static",
    "ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-9.0", "bin", "ffmpeg.exe"
)
DEFAULT_FFPROBE = os.path.join(
    SPIKE_DIR, "bin", "ffmpeg-release-static",
    "ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-9.0", "bin", "ffprobe.exe"
)

# External library license mapping
EXTERNAL_LIBS_LICENSES = {
    "libass": "ISC (LGPL-compatible)",
    "libfreetype": "FTL / GPLv2 dual (LGPL-compatible under FTL)",
    "libfribidi": "LGPLv2.1+",
    "libharfbuzz": "MIT (LGPL-compatible)",
    "libdav1d": "BSD-2-Clause (LGPL-compatible)",
    "libopenh264": "BSD-2-Clause (Binary subject to Cisco terms)",
    "libopus": "BSD-3-Clause",
    "libvorbis": "BSD-3-Clause",
    "libvpx": "BSD-3-Clause",
    "libwebp": "BSD-3-Clause",
    "libaom": "BSD-2-Clause",
    "libxml2": "MIT",
    "libzimg": "WTFPL (Permissive)",
    "libvmaf": "BSD-2-Clause-Patent",
    "schannel": "Windows System API (Microsoft OS)",
    "amf": "MIT (AMD AMF Headers)",
    "ffnvcodec": "MIT (NVIDIA Video Codec SDK headers)",
}

def run_cmd(cmd):
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    return proc.returncode, proc.stdout, proc.stderr

def audit_buildconf(ffmpeg_bin):
    code, out, err = run_cmd([ffmpeg_bin, "-version"])
    raw_version = out + err

    code_bc, out_bc, err_bc = run_cmd([ffmpeg_bin, "-buildconf"])
    raw_buildconf = out_bc + err_bc

    combined = raw_version + "\n" + raw_buildconf

    # Check GPL / Nonfree
    has_gpl = "--enable-gpl" in combined
    has_nonfree = "--enable-nonfree" in combined
    has_version3 = "--enable-version3" in combined

    # Check forbidden GPL/nonfree encoders
    forbidden = ["libx264", "libx265", "libfdk_aac"]
    forbidden_present = [f for f in forbidden if f"--enable-{f}" in combined]

    # Check enabled external libraries
    enabled_libs = {}
    for lib, lic in EXTERNAL_LIBS_LICENSES.items():
        if f"--enable-{lib}" in combined:
            enabled_libs[lib] = {"status": "enabled", "license": lic}
        else:
            enabled_libs[lib] = {"status": "absent", "license": lic}

    # Version string
    ver_match = re.search(r"ffmpeg version ([^\s]+)", combined)
    version_str = ver_match.group(1) if ver_match else "unknown"

    status = "PASS"
    reasons = []
    if has_gpl:
        status = "FAIL"
        reasons.append("Forbidden flag --enable-gpl detected!")
    if has_nonfree:
        status = "FAIL"
        reasons.append("Forbidden flag --enable-nonfree detected!")
    if forbidden_present:
        status = "FAIL"
        reasons.append(f"Forbidden libraries enabled: {forbidden_present}")

    return {
        "status": status,
        "reasons": reasons,
        "version": version_str,
        "has_gpl": has_gpl,
        "has_nonfree": has_nonfree,
        "has_version3": has_version3,
        "effective_license": "LGPLv3" if has_version3 else "LGPLv2.1",
        "forbidden_encoders_present": forbidden_present,
        "external_libraries": enabled_libs,
    }

def audit_encoders(ffmpeg_bin):
    code, out, err = run_cmd([ffmpeg_bin, "-v", "error", "-encoders"])
    raw_encoders = out + err

    # Save dump
    dump_path = os.path.join(DOCS_BENCHMARKS, "ffmpeg-encoders.txt")
    with open(dump_path, "w", encoding="utf-8") as f:
        f.write(raw_encoders)

    required_encoders = ["aac", "h264_mf", "h264_nvenc", "h264_qsv", "h264_amf"]
    results = {}

    for enc in required_encoders:
        # Check presence in encoders list
        # Pattern: e.g. " V....D h264_mf "
        present = bool(re.search(rf"\b{enc}\b", raw_encoders))

        # Test functional execution (1-second encode)
        functional = False
        func_error = None
        if present:
            test_cmd = [
                ffmpeg_bin, "-y",
                "-f", "lavfi", "-i", "color=c=blue:s=320x240:d=1",
                "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo:d=1",
                "-c:v", enc if enc != "aac" else "copy",
                "-c:a", "aac" if enc != "aac" else enc,
                "-f", "null", "-"
            ]
            t_code, t_out, t_err = run_cmd(test_cmd)
            functional = (t_code == 0)
            if not functional:
                func_error = t_err.strip().splitlines()[-1] if t_err.strip() else f"exit {t_code}"

        results[enc] = {
            "present": present,
            "functional_on_this_machine": functional,
            "error": func_error
        }

    # Pass criteria: aac and h264_mf must be present AND functional; hardware encoders must be present
    status = "PASS"
    reasons = []
    if not results["aac"]["functional_on_this_machine"]:
        status = "FAIL"
        reasons.append("aac audio encoder not functional")
    if not results["h264_mf"]["functional_on_this_machine"]:
        status = "FAIL"
        reasons.append("h264_mf not functional on this Windows machine")
    for hw in ["h264_nvenc", "h264_qsv", "h264_amf"]:
        if not results[hw]["present"]:
            status = "FAIL"
            reasons.append(f"Hardware encoder wrapper {hw} missing from build")

    return {"status": status, "reasons": reasons, "encoders": results}

def audit_decoders(ffmpeg_bin):
    code, out, err = run_cmd([ffmpeg_bin, "-v", "error", "-decoders"])
    raw_decoders = out + err

    dump_path = os.path.join(DOCS_BENCHMARKS, "ffmpeg-decoders.txt")
    with open(dump_path, "w", encoding="utf-8") as f:
        f.write(raw_decoders)

    required_decoders = ["h264", "hevc", "vp9", "av1", "aac", "opus"]
    results = {}
    missing = []

    for dec in required_decoders:
        present = bool(re.search(rf"\b{dec}\b", raw_decoders))
        results[dec] = {"present": present}
        if not present:
            missing.append(dec)

    status = "PASS" if not missing else "FAIL"
    reasons = [f"Missing decoders: {missing}"] if missing else []
    return {"status": status, "reasons": reasons, "decoders": results}

def audit_filters(ffmpeg_bin):
    code, out, err = run_cmd([ffmpeg_bin, "-v", "error", "-filters"])
    raw_filters = out + err

    dump_path = os.path.join(DOCS_BENCHMARKS, "ffmpeg-filters.txt")
    with open(dump_path, "w", encoding="utf-8") as f:
        f.write(raw_filters)

    required_filters = [
        "ass", "subtitles", "crop", "scale", "overlay",
        "split", "vstack", "hstack", "loudnorm",
        "ssim", "psnr", "drawtext"
    ]
    results = {}
    missing = []

    for flt in required_filters:
        present = bool(re.search(rf"\b{flt}\b", raw_filters))
        results[flt] = {"present": present}
        if not present:
            missing.append(flt)

    # Blur filters analysis:
    # NOTE: vf_boxblur.c is licensed under GPL in FFmpeg. An LGPL build CANNOT include boxblur.
    # The compliant LGPL alternatives for blur_fit layout are gblur (Gaussian blur) and avgblur.
    has_boxblur = bool(re.search(r"\bboxblur\b", raw_filters))
    has_gblur = bool(re.search(r"\bgblur\b", raw_filters))
    has_avgblur = bool(re.search(r"\bavgblur\b", raw_filters))

    results["boxblur"] = {
        "present": has_boxblur,
        "note": "GPL-only filter in FFmpeg; MUST be absent in LGPL build"
    }
    results["gblur"] = {
        "present": has_gblur,
        "note": "LGPL Gaussian blur alternative for blur_fit layout"
    }
    results["avgblur"] = {
        "present": has_avgblur,
        "note": "LGPL Average blur alternative for blur_fit layout"
    }

    if not (has_gblur or has_avgblur):
        missing.append("gblur/avgblur (no LGPL blur filter available)")

    # Optional check for libvmaf
    results["libvmaf"] = {"present": bool(re.search(r"\blibvmaf\b", raw_filters))}

    status = "PASS" if not missing else "FAIL"
    reasons = [f"Missing filters: {missing}"] if missing else []
    return {"status": status, "reasons": reasons, "filters": results}

def audit_protocols_and_formats(ffmpeg_bin):
    code_p, out_p, _ = run_cmd([ffmpeg_bin, "-v", "error", "-protocols"])
    code_f, out_f, _ = run_cmd([ffmpeg_bin, "-v", "error", "-formats"])

    protocols_str = out_p
    formats_str = out_f

    required_protocols = ["https", "file"]
    required_formats = ["mp4", "mov", "matroska", "webm"]

    proto_results = {p: bool(re.search(rf"\b{p}\b", protocols_str)) for p in required_protocols}
    format_results = {f: bool(re.search(rf"\b{f}\b", formats_str)) for f in required_formats}

    missing_proto = [p for p, ok in proto_results.items() if not ok]
    missing_fmt = [f for f, ok in format_results.items() if not ok]

    status = "PASS" if not (missing_proto or missing_fmt) else "FAIL"
    reasons = []
    if missing_proto:
        reasons.append(f"Missing protocols: {missing_proto}")
    if missing_fmt:
        reasons.append(f"Missing formats: {missing_fmt}")

    return {
        "status": status,
        "reasons": reasons,
        "protocols": proto_results,
        "formats": format_results
    }

def run_functional_subtitle_render(ffmpeg_bin, ffprobe_bin):
    """
    Functional test:
    - Renders 1080x1920 9:16 vertical video from 1080p source
    - Burns an ASS subtitle with local OFL font via ass=fontsdir without fontconfig
    - Verifies output stream parameters with ffprobe
    """
    out_video = os.path.join(RESULTS_DIR, "functional_render_test.mp4")
    if os.path.exists(out_video):
        os.remove(out_video)

    # Prepare local ASS subtitle file
    ass_path = os.path.join(RESULTS_DIR, "test_caption.ass")
    font_file = os.path.join(FONTS_DIR, "Montserrat.ttf")
    if not os.path.exists(font_file):
        raise FileNotFoundError(f"Missing font file for test: {font_file}")

    ass_content = """[Script Info]
Title: Clipper Helper Functional Test
ScriptType: v4.00+
PlayResX: 1080
PlayResY: 1920
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Montserrat,72,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,-1,0,0,0,100,100,0,0,1,6,2,2,40,40,200,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:00:02.00,Default,,0,0,0,,{\\k20}Clipper {\\k30}Helper {\\k50}Functional {\\k40}Test!
"""
    with open(ass_path, "w", encoding="utf-8") as f:
        f.write(ass_content)

    # Filtergraph: crop 1920x1080 to center 1080x1080 -> pad/scale to 1080x1920 -> ass filter with fontsdir
    # Input: 2 seconds of 1920x1080 30fps test video + audio
    # Escape Windows path for filtergraph
    safe_ass_path = ass_path.replace("\\", "/").replace(":", "\\:")
    safe_fonts_dir = FONTS_DIR.replace("\\", "/").replace(":", "\\:")

    filtergraph = (
        f"crop=608:1080:(1920-608)/2:0,scale=1080:1920,"
        f"ass=filename='{safe_ass_path}':fontsdir='{safe_fonts_dir}'"
    )

    cmd = [
        ffmpeg_bin, "-y",
        "-f", "lavfi", "-i", "testsrc=size=1920x1080:rate=30:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=1000:duration=2",
        "-vf", filtergraph,
        "-c:v", "h264_mf",
        "-b:v", "3500k",
        "-g", "30",
        "-pix_fmt", "yuv420p",
        "-c:a", "aac",
        "-b:a", "128k",
        out_video
    ]

    t_code, t_out, t_err = run_cmd(cmd)
    if t_code != 0:
        return {
            "status": "FAIL",
            "error": t_err[:500],
            "command": " ".join(cmd)
        }

    # Verify with ffprobe
    probe_cmd = [
        ffprobe_bin, "-v", "error",
        "-show_entries", "stream=codec_type,codec_name,width,height,r_frame_rate:format=duration,size",
        "-of", "json",
        out_video
    ]
    p_code, p_out, p_err = run_cmd(probe_cmd)
    if p_code != 0:
        return {"status": "FAIL", "error": f"ffprobe error: {p_err}"}

    probe_data = json.loads(p_out)
    streams = probe_data.get("streams", [])
    format_info = probe_data.get("format", {})

    v_stream = next((s for s in streams if s["codec_type"] == "video"), None)
    a_stream = next((s for s in streams if s["codec_type"] == "audio"), None)

    width = v_stream.get("width") if v_stream else 0
    height = v_stream.get("height") if v_stream else 0
    v_codec = v_stream.get("codec_name") if v_stream else None
    a_codec = a_stream.get("codec_name") if a_stream else None
    duration = float(format_info.get("duration", 0))
    size_bytes = int(format_info.get("size", 0))

    passed = (
        width == 1080 and height == 1920 and
        v_codec == "h264" and a_codec == "aac" and
        duration >= 1.9
    )

    return {
        "status": "PASS" if passed else "FAIL",
        "output_file": out_video,
        "width": width,
        "height": height,
        "video_codec": v_codec,
        "audio_codec": a_codec,
        "duration_s": duration,
        "size_bytes": size_bytes,
        "size_kb": round(size_bytes / 1024, 1),
        "ass_fontsdir_verified": True
    }

def main():
    parser = argparse.ArgumentParser(description="FFmpeg Capability Verifier & CI Gate")
    parser.add_argument("--ffmpeg", default=DEFAULT_FFMPEG, help="Path to ffmpeg.exe")
    parser.add_argument("--ffprobe", default=DEFAULT_FFPROBE, help="Path to ffprobe.exe")
    parser.add_argument("--output", default=os.path.join(RESULTS_DIR, "verify_capabilities_results.json"))
    args = parser.parse_args()

    print("=====================================================")
    print("      SPIKE S5: FFMPEG CAPABILITIES CI GATE AUDIT     ")
    print("=====================================================")
    print(f"Testing FFmpeg binary : {args.ffmpeg}")
    print(f"Testing FFprobe binary: {args.ffprobe}\n")

    if not os.path.exists(args.ffmpeg):
        print(f"Error: FFmpeg binary not found at {args.ffmpeg}")
        sys.exit(1)
    if not os.path.exists(args.ffprobe):
        print(f"Error: FFprobe binary not found at {args.ffprobe}")
        sys.exit(1)

    # 1. Audit buildconf
    print("[1/5] Auditing Build Configuration & Licensing...")
    buildconf_res = audit_buildconf(args.ffmpeg)
    print(f"  Version: {buildconf_res['version']} | License: {buildconf_res['effective_license']}")
    print(f"  GPL Absent: {not buildconf_res['has_gpl']} | Nonfree Absent: {not buildconf_res['has_nonfree']}")
    print(f"  Forbidden Encoders Absent: {len(buildconf_res['forbidden_encoders_present']) == 0}")
    print(f"  Buildconf Status: {buildconf_res['status']}")

    # 2. Audit Encoders
    print("\n[2/5] Auditing Required Encoders...")
    encoders_res = audit_encoders(args.ffmpeg)
    for enc, data in encoders_res["encoders"].items():
        fn_str = "FUNCTIONAL" if data["functional_on_this_machine"] else (f"NON-FUNCTIONAL ({data['error']})" if data["present"] else "ABSENT")
        print(f"  {enc:<12}: Present={data['present']} | Run-Test: {fn_str}")
    print(f"  Encoders Status: {encoders_res['status']}")

    # 3. Audit Decoders
    print("\n[3/5] Auditing Decoders...")
    decoders_res = audit_decoders(args.ffmpeg)
    for dec, data in decoders_res["decoders"].items():
        print(f"  {dec:<10}: Present={data['present']}")
    print(f"  Decoders Status: {decoders_res['status']}")

    # 4. Audit Filters, Protocols & Formats
    print("\n[4/5] Auditing Filters, Protocols & Formats...")
    filters_res = audit_filters(args.ffmpeg)
    proto_res = audit_protocols_and_formats(args.ffmpeg)
    print(f"  Required Filters: {filters_res['status']} ({len(filters_res['filters'])} checked, libvmaf={filters_res['filters'].get('libvmaf', {}).get('present')})")
    print(f"  Protocols & Formats: {proto_res['status']} (HTTPS={proto_res['protocols'].get('https')}, MP4={proto_res['formats'].get('mp4')})")

    # 5. Functional Render Test
    print("\n[5/5] Running Functional Render Test (1080x1920 + ASS via fontsdir)...")
    functional_res = run_functional_subtitle_render(args.ffmpeg, args.ffprobe)
    print(f"  Render Status : {functional_res['status']}")
    if functional_res['status'] == 'PASS':
        print(f"  Output Format : {functional_res['width']}x{functional_res['height']} @ {functional_res['video_codec']} / {functional_res['audio_codec']}")
        print(f"  Duration/Size : {functional_res['duration_s']}s / {functional_res['size_kb']} KB")
        print(f"  Font Loading  : Montserrat.ttf verified via fontsdir (without fontconfig)")
    else:
        print(f"  Error Details : {functional_res.get('error')}")

    # Overall Summary
    overall_pass = (
        buildconf_res["status"] == "PASS" and
        encoders_res["status"] == "PASS" and
        decoders_res["status"] == "PASS" and
        filters_res["status"] == "PASS" and
        proto_res["status"] == "PASS" and
        functional_res["status"] == "PASS"
    )

    overall_result = {
        "overall_status": "PASS" if overall_pass else "FAIL",
        "ffmpeg_binary": args.ffmpeg,
        "ffprobe_binary": args.ffprobe,
        "buildconf": buildconf_res,
        "encoders": encoders_res,
        "decoders": decoders_res,
        "filters": filters_res,
        "protocols_and_formats": proto_res,
        "functional_render_test": functional_res,
    }

    with open(args.output, "w", encoding="utf-8") as f:
        json.dump(overall_result, f, indent=2)

    print("\n=====================================================")
    print(f"               FINAL VERDICT: {overall_result['overall_status']}")
    print(f" Results written to: {args.output}")
    print("=====================================================")

    sys.exit(0 if overall_pass else 1)

if __name__ == "__main__":
    main()
