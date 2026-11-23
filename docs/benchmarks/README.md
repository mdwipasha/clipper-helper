# Benchmark Reference & Hardware Environments

## 1. Reference Machine R1 (and Provenance Audit)

Benchmarks in this directory (including Spike S1, S1b, S1c, S3b, and S4b) were conducted on the following primary development and reference workstation:

| Attribute | Specification |
|---|---|
| **Identifier** | **Reference Machine R1** |
| **Model** | Lenovo ThinkPad (20X8S07R00) |
| **Processor (CPU)** | AMD Ryzen 5 PRO 5650U with Radeon Graphics (6 Cores / 12 Threads, 2.30 GHz base, AVX2) |
| **System Memory (RAM)** | 32.0 GB DDR4 (31,566 MB physical) |
| **Graphics (GPU)** | AMD Radeon(TM) Graphics (Integrated Vega 7 iGPU, 1 GB shared VRAM) |
| **Operating System** | Microsoft Windows 11 Pro 64-bit (OS Build 26200) |
| **Power Profile** | Windows Balanced plan, connected to AC line power (100% full charge) |
| **Network Interface** | MediaTek MT7921 Wi-Fi 6 802.11ax PCIe Adapter |
| **Internet Connection** | AS38482 Biznet Networks (Residential/commercial optical fiber, Jakarta, Indonesia) |

### 1.1 Host Name Provenance & Machine Identity
Two distinct Windows Host Names were captured across recent benchmark runs on identical hardware:
- **`DESKTOP-787UCLP`** (recorded in Spike S3b system probe, 2026-11-21)
- **`DESKTOP-OO9CA87`** (recorded in Spike S4b system probe, 2026-11-21)

> [!NOTE]
> **Status:** To be confirmed by the user whether these logs represent **one renamed machine** or **two identical hardware units (R1 / R1b)**. Both environments share the exact same hardware configuration (Lenovo 20X8S07R00, Ryzen 5 PRO 5650U, Vega 7 iGPU, 32 GB RAM, Windows 11 Build 26200).

---

## 2. Benchmark Reporting Rules (Mandatory)

To ensure full scientific reproducibility, eliminate ambiguity, and prevent unverified copy-paste errors:
1. **Raw System Probes:** Every benchmark report must include verbatim output from `systeminfo`, `Win32_Processor`, `Win32_VideoController`, and `powercfg /getactivescheme`.
2. **Explicit Host & Date:** Every benchmark report must state the exact Windows Host Name, user, execution date, and AC/battery state.
3. **Measured vs Placeholder:** All data rows must be explicitly labeled as **Measured** (with sample size and hardware target) or **UNTESTED / PLACEHOLDER**. Unmeasured placeholder numbers must never be labeled as "verified".
4. **Benchmark Variance & Standard Protocol (≥5 Runs, Median + IQR):**
   Speech-to-text inference and video encoding throughput exhibit significant run-to-run variance on mobile/desktop hardware due to CPU thermal boost states, Windows background services, and dynamic clock scaling. For instance, the identical 60 s `podcast_mixed.wav` job with `small-q5_1` took 53.2 s in the S3c model matrix run versus 37.7 s in the S3d baseline run (a 1.41× spread on the same machine).
   - **Protocol:** Benchmark runs must execute **≥5 runs per condition**, reporting the **median and IQR** (Interquartile Range) rather than single-run numbers.
   - **Environment Recording:** Every report must record the active Windows power plan (`powercfg /getactivescheme`), AC vs battery state, background system load (CPU % and RAM before test launch), and CPU core clock / throttling status if available.
   - **Priors as Ranges:** Rate priors in architecture and ETA specs must be expressed as **RANGES** (e.g. 1.1–2.2× real-time), never single-point constants.

---

## 3. Policy on Minimum Supported Specification

> [!IMPORTANT]
> **R1 does NOT define the minimum supported specification.**
> Benchmark measurements obtained on R1 serve as an engineering baseline for development and performance comparison. The final minimum supported hardware floor will be established only after:
> 1. Testing on lower hardware profiles (specifically 4 CPU threads, 8 GB RAM, and older integrated graphics).
> 2. Analyzing real-world performance telemetry collected during the M6 closed beta.
