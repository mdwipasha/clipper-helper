#!/usr/bin/env node
/**
 * tools/src/fetch-pinned-components.js
 *
 * Downloads the PINNED component binaries and speech models from the S1/S3/S5 inventory into a
 * git-ignored folder (`tools/dev-fixtures/real/`), strictly verifies every SHA-256 checksum,
 * repackages ZIP bundles with stripped/clean runtime files, and calls `manifest-signer.js` to
 * build & sign DEV manifests served by `dev-server.js`.
 *
 * Generated manifests inside `tools/dev-fixtures/real/`:
 * - `manifest.json` (+ .sig, seq 1): Pinned LGPL FFmpeg, yt-dlp, qjs, whisper-cli, models
 * - `manifest-gpl-ffmpeg.json` (+ .sig, seq 2): Includes real S1 gyan GPL FFmpeg build for refusal testing
 * - `manifest-v2.json` (+ .sig, seq 3): Includes a second version of qjs & ffmpeg for rollback/forward testing
 */

import * as crypto from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';
import * as zlib from 'node:zlib';

import {
  buildManifest,
  computeSha256,
  generateDevKeyPair,
  signManifestBytes,
} from './manifest-signer.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, '../..');
const DEFAULT_OUT_DIR = path.join(REPO_ROOT, 'tools', 'dev-fixtures', 'real');
const DEV_KEYS_DIR = path.join(REPO_ROOT, '.dev-keys');

// ── Pinned S1 / S3 / S5 Inventory ───────────────────────────────────────────

export const PINNED_COMPONENTS = {
  ffmpegLgplZip: {
    component: 'ffmpeg',
    version: 'n9.0.2-58-g8c7ef49002-20261119',
    url: 'https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-19-12-39/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip',
    cacheName: 'ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip',
    sha256: '1fdb837def412ad7cc019874b6310156f40179bd30df58d041a82413f800a331',
    localSpikePath: 'spikes/s5-ffmpeg/bin/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip',
  },
  ffmpegGplZip: {
    component: 'ffmpeg',
    version: '9.0.2-essentials-gpl',
    url: 'https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip',
    cacheName: 'ffmpeg-9.0.2-essentials_build.zip',
    sha256: '60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba',
    localSpikePath: 'spikes/s1-downloader/bin/ffmpeg-release-essentials.zip',
  },
  ytDlp: {
    component: 'yt-dlp',
    version: '2026.10.26',
    url: 'https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp.exe',
    cacheName: 'yt-dlp.exe',
    sha256: 'f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d',
    localSpikePath: 'spikes/s1-downloader/bin/yt-dlp.exe',
  },
  qjs: {
    component: 'qjs',
    version: 'v0.18.0',
    url: 'https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-windows-x86_64.exe',
    cacheName: 'qjs.exe',
    sha256: 'ff03398d74ea6e6bd3a8e58058f7003225abcac229d7d57775b7aaae812a49c7',
    localSpikePath: 'spikes/s1-downloader/bin/qjs.exe',
  },
  whisperCliZip: {
    component: 'whisper-cli',
    version: '1.9.5',
    url: 'https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip',
    cacheName: 'whisper-bin-x64.zip',
    sha256: '6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb',
    localSpikePath: 'spikes/s3-speech/bin/whisper-bin-x64.zip',
  },
  sileroVad: {
    component: 'ggml-silero-v6.2.0.bin',
    version: 'silero-v6.2.0-2aa269b7',
    url: 'https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin',
    cacheName: 'ggml-silero-v6.2.0.bin',
    sha256: '2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987',
    localSpikePath: 'spikes/s3-speech/models/ggml-silero-v6.2.0.bin',
  },
  whisperModelBase: {
    component: 'ggml-base.bin',
    version: 'whisper-base-60ed5bc3',
    url: 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin',
    cacheName: 'ggml-base.bin',
    sha256: '60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe',
    localSpikePath: 'spikes/s3-speech/models/ggml-base.bin',
  },
  whisperModelSmallQ5_1: {
    component: 'ggml-small-q5_1.bin',
    version: 'whisper-small-q5_1-ae85e4a9',
    url: 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin',
    cacheName: 'ggml-small-q5_1.bin',
    sha256: 'ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb',
    localSpikePath: 'spikes/s3-speech/models/ggml-small-q5_1.bin',
  },
};

// ── Minimal Pure-Node ZIP Reader / Writer (Stored & Deflate) ────────────────

const CRC_TABLE = (() => {
  const tbl = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) {
      c = (c & 1) !== 0 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    tbl[i] = c >>> 0;
  }
  return tbl;
})();

export function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

/**
 * Reads all entries from a ZIP buffer (supports Stored=0 and Deflate=8).
 * Returns array of `{ name: string, data: Buffer }`.
 */
export function readZipEntries(zipBuffer) {
  const len = zipBuffer.length;
  const minStart = Math.max(0, len - (22 + 65535));
  let eocdOffset = -1;
  for (let i = len - 22; i >= minStart; i--) {
    if (zipBuffer.readUInt32LE(i) === 0x06054b50) {
      const commentLen = zipBuffer.readUInt16LE(i + 20);
      if (i + 22 + commentLen === len) {
        eocdOffset = i;
        break;
      }
    }
  }
  if (eocdOffset === -1) {
    throw new Error('Invalid ZIP: End of Central Directory not found');
  }

  const totalEntries = zipBuffer.readUInt16LE(eocdOffset + 10);
  const cdOffset = zipBuffer.readUInt32LE(eocdOffset + 16);

  const entries = [];
  let cursor = cdOffset;
  for (let idx = 0; idx < totalEntries; idx++) {
    if (zipBuffer.readUInt32LE(cursor) !== 0x02014b50) {
      throw new Error(`Invalid Central Directory header at offset ${cursor}`);
    }
    const method = zipBuffer.readUInt16LE(cursor + 10);
    const compressedSize = zipBuffer.readUInt32LE(cursor + 20);
    const uncompressedSize = zipBuffer.readUInt32LE(cursor + 24);
    const nameLen = zipBuffer.readUInt16LE(cursor + 28);
    const extraLen = zipBuffer.readUInt16LE(cursor + 30);
    const commentLen = zipBuffer.readUInt16LE(cursor + 32);
    const localHeaderOffset = zipBuffer.readUInt32LE(cursor + 42);

    const name = zipBuffer.subarray(cursor + 46, cursor + 46 + nameLen).toString('utf8');
    cursor += 46 + nameLen + extraLen + commentLen;

    if (name.endsWith('/')) {
      continue;
    }

    if (zipBuffer.readUInt32LE(localHeaderOffset) !== 0x04034b50) {
      throw new Error(`Invalid Local File Header at offset ${localHeaderOffset}`);
    }
    const lfhNameLen = zipBuffer.readUInt16LE(localHeaderOffset + 26);
    const lfhExtraLen = zipBuffer.readUInt16LE(localHeaderOffset + 28);
    const dataStart = localHeaderOffset + 30 + lfhNameLen + lfhExtraLen;
    const rawCompressed = zipBuffer.subarray(dataStart, dataStart + compressedSize);

    let data;
    if (method === 0) {
      data = Buffer.from(rawCompressed);
    } else if (method === 8) {
      data = zlib.inflateRawSync(rawCompressed);
    } else {
      throw new Error(`Unsupported ZIP compression method ${method} for '${name}'`);
    }

    if (data.length !== uncompressedSize) {
      throw new Error(
        `Size mismatch for '${name}': expected ${uncompressedSize}, got ${data.length}`
      );
    }
    entries.push({ name, data });
  }
  return entries;
}

/**
 * Writes a deterministic clean ZIP archive from `{ name: string, data: Buffer }[]`.
 * Uses raw Deflate (method 8) for binaries/DLLs and clears platform/symlink external attributes.
 */
export function buildCleanZip(items) {
  const localParts = [];
  const cdParts = [];
  let offset = 0;

  for (const item of items) {
    const nameBuf = Buffer.from(item.name, 'utf8');
    const crc = crc32(item.data);
    const deflated = zlib.deflateRawSync(item.data, { level: 6 });
    const useDeflate = deflated.length < item.data.length;
    const method = useDeflate ? 8 : 0;
    const payload = useDeflate ? deflated : item.data;

    // Local File Header (30 bytes)
    const lfh = Buffer.alloc(30);
    lfh.writeUInt32LE(0x04034b50, 0);
    lfh.writeUInt16LE(20, 4); // version needed 2.0
    lfh.writeUInt16LE(0x0800, 6); // UTF-8 filename flag
    lfh.writeUInt16LE(method, 8);
    lfh.writeUInt16LE(0, 10);
    lfh.writeUInt16LE(0x5421, 12); // deterministic DOS date
    lfh.writeUInt32LE(crc, 14);
    lfh.writeUInt32LE(payload.length, 18);
    lfh.writeUInt32LE(item.data.length, 22);
    lfh.writeUInt16LE(nameBuf.length, 26);
    lfh.writeUInt16LE(0, 28);

    localParts.push(lfh, nameBuf, payload);

    // Central Directory Header (46 bytes)
    const cd = Buffer.alloc(46);
    cd.writeUInt32LE(0x02014b50, 0);
    cd.writeUInt16LE(0x031e, 4); // version made by DOS/POSIX compat
    cd.writeUInt16LE(20, 6); // version needed
    cd.writeUInt16LE(0x0800, 8); // UTF-8 flag
    cd.writeUInt16LE(method, 10);
    cd.writeUInt16LE(0, 12);
    cd.writeUInt16LE(0x5421, 14);
    cd.writeUInt32LE(crc, 16);
    cd.writeUInt32LE(payload.length, 20);
    cd.writeUInt32LE(item.data.length, 24);
    cd.writeUInt16LE(nameBuf.length, 28);
    cd.writeUInt16LE(0, 30);
    cd.writeUInt16LE(0, 32);
    cd.writeUInt16LE(0, 34);
    cd.writeUInt16LE(0, 36);
    cd.writeUInt32LE(0x20, 38); // regular DOS archive attribute, no symlink/reparse point
    cd.writeUInt32LE(offset, 42);

    cdParts.push(cd, nameBuf);
    offset += lfh.length + nameBuf.length + payload.length;
  }

  const cdBuf = Buffer.concat(cdParts);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4);
  eocd.writeUInt16LE(0, 6);
  eocd.writeUInt16LE(items.length, 8);
  eocd.writeUInt16LE(items.length, 10);
  eocd.writeUInt32LE(cdBuf.length, 12);
  eocd.writeUInt32LE(offset, 16);
  eocd.writeUInt16LE(0, 20);

  return Buffer.concat([...localParts, cdBuf, eocd]);
}

// ── Download & SHA-256 Verification ─────────────────────────────────────────

function sha256FileStreamSync(filePath) {
  const hash = crypto.createHash('sha256');
  const fd = fs.openSync(filePath, 'r');
  const buf = Buffer.alloc(1024 * 1024);
  try {
    let bytesRead;
    while ((bytesRead = fs.readSync(fd, buf, 0, buf.length, null)) > 0) {
      hash.update(buf.subarray(0, bytesRead));
    }
  } finally {
    fs.closeSync(fd);
  }
  return hash.digest('hex');
}

async function ensureVerifiedFile(item, cacheDir) {
  fs.mkdirSync(cacheDir, { recursive: true });
  const cachePath = path.join(cacheDir, item.cacheName);

  // 1. Check already cached file
  if (fs.existsSync(cachePath)) {
    const existingSha = sha256FileStreamSync(cachePath);
    if (existingSha === item.sha256) {
      console.log(`  [cache-ok] ${item.cacheName} (${item.sha256.slice(0, 12)}…)`);
      return cachePath;
    }
    console.warn(`  [cache-mismatch] Removing invalid cached file: ${item.cacheName}`);
    fs.unlinkSync(cachePath);
  }

  // 2. Check verified local spike artifact if present on workstation
  if (item.localSpikePath) {
    const spikeAbs = path.join(REPO_ROOT, item.localSpikePath);
    if (fs.existsSync(spikeAbs)) {
      const spikeSha = sha256FileStreamSync(spikeAbs);
      if (spikeSha === item.sha256) {
        console.log(
          `  [spike-verified] Copying verified ${item.cacheName} from ${item.localSpikePath}`
        );
        fs.copyFileSync(spikeAbs, `${cachePath}.tmp`);
        fs.renameSync(`${cachePath}.tmp`, cachePath);
        return cachePath;
      }
    }
  }

  // 3. Download from pinned upstream URL with atomic .tmp -> rename & hash verify
  console.log(`  [download] ${item.url}`);
  const resp = await fetch(item.url, {
    headers: { 'User-Agent': 'ClipperHelper-FetchPinned/1.0' },
  });
  if (!resp.ok || !resp.body) {
    throw new Error(`HTTP ${resp.status} downloading ${item.url}`);
  }
  const tmpPath = `${cachePath}.tmp`;
  await pipeline(Readable.fromWeb(resp.body), fs.createWriteStream(tmpPath));

  const actualSha = sha256FileStreamSync(tmpPath);
  if (actualSha !== item.sha256) {
    fs.unlinkSync(tmpPath);
    throw new Error(
      `SHA-256 mismatch for ${item.cacheName}: expected ${item.sha256}, got ${actualSha}`
    );
  }
  fs.renameSync(tmpPath, cachePath);
  console.log(`  [verified] ${item.cacheName} SHA-256 OK`);
  return cachePath;
}

function repackageZipFiltered(sourceZipPath, destZipPath, selectorFn) {
  fs.mkdirSync(path.dirname(destZipPath), { recursive: true });
  const zipBuf = fs.readFileSync(sourceZipPath);
  const rawEntries = readZipEntries(zipBuf);
  const selected = [];
  for (const entry of rawEntries) {
    const targetName = selectorFn(entry.name);
    if (targetName) {
      selected.push({ name: targetName, data: entry.data });
    }
  }
  if (selected.length === 0) {
    throw new Error(`No entries matched filter in ${sourceZipPath}`);
  }

  const packed = buildCleanZip(selected);
  fs.writeFileSync(`${destZipPath}.tmp`, packed);
  fs.renameSync(`${destZipPath}.tmp`, destZipPath);

  return {
    zipPath: destZipPath,
    size_bytes: packed.length,
    sha256: computeSha256(packed),
    files: selected.map((f) => ({
      path: f.name,
      sha256: computeSha256(f.data),
      size_bytes: f.data.length,
    })),
  };
}

function writeAndSignManifest(filePath, manifestObj, privKeyHex, kid) {
  const bytes = Buffer.from(JSON.stringify(manifestObj, null, 2), 'utf8');
  fs.writeFileSync(filePath, bytes);
  const sig = signManifestBytes(bytes, privKeyHex, kid);
  fs.writeFileSync(`${filePath}.sig`, JSON.stringify(sig, null, 2));
}

export async function runFetchPinnedComponents({
  outDir = DEFAULT_OUT_DIR,
  baseUrl = 'http://127.0.0.1:8080/real',
  skipModels = false,
  skipGpl = false,
} = {}) {
  fs.mkdirSync(outDir, { recursive: true });
  fs.mkdirSync(DEV_KEYS_DIR, { recursive: true });

  // Ensure dev signing keypair matches .dev-keys/clipper-manifest-dev
  const kid = 'clipper-manifest-dev';
  let privKeyPath = path.join(DEV_KEYS_DIR, `${kid}.priv.hex`);
  if (!fs.existsSync(privKeyPath)) {
    const generated = generateDevKeyPair(kid, DEV_KEYS_DIR);
    privKeyPath = generated.privPath;
  }
  const privKeyHex = fs.readFileSync(privKeyPath, 'utf8');

  const cacheDir = path.join(outDir, 'pinned-cache');

  console.log('=== Verifying & packaging pinned S1/S3/S5 components ===');
  const ffmpegLgplRaw = await ensureVerifiedFile(PINNED_COMPONENTS.ffmpegLgplZip, cacheDir);
  const ytDlpRaw = await ensureVerifiedFile(PINNED_COMPONENTS.ytDlp, cacheDir);
  const qjsRaw = await ensureVerifiedFile(PINNED_COMPONENTS.qjs, cacheDir);
  const whisperRaw = await ensureVerifiedFile(PINNED_COMPONENTS.whisperCliZip, cacheDir);

  // Package FFmpeg LGPL Shared (ffmpeg.exe, ffprobe.exe + 7 DLLs; exclude ffplay/doc/lib)
  const ffmpegKeep = new Set([
    'ffmpeg.exe',
    'ffprobe.exe',
    'avcodec-63.dll',
    'avdevice-63.dll',
    'avfilter-12.dll',
    'avformat-63.dll',
    'avutil-61.dll',
    'swresample-7.dll',
    'swscale-10.dll',
  ]);
  const ffmpegVer = PINNED_COMPONENTS.ffmpegLgplZip.version;
  const ffmpegPkg = repackageZipFiltered(
    ffmpegLgplRaw,
    path.join(outDir, 'components', 'ffmpeg', ffmpegVer, 'ffmpeg.zip'),
    (entryName) => {
      const idx = entryName.lastIndexOf('/bin/');
      if (idx === -1) return null;
      const base = entryName.slice(idx + 5);
      return ffmpegKeep.has(base) ? base : null;
    }
  );

  // Package whisper-cli (whisper-cli.exe, whisper.dll, ggml*.dll)
  const whisperVer = PINNED_COMPONENTS.whisperCliZip.version;
  const whisperPkg = repackageZipFiltered(
    whisperRaw,
    path.join(outDir, 'components', 'whisper-cli', whisperVer, 'whisper-cli.zip'),
    (entryName) => {
      if (!entryName.startsWith('Release/')) return null;
      const base = entryName.slice('Release/'.length);
      if (base.includes('/')) return null;
      if (
        base === 'whisper-cli.exe' ||
        base === 'whisper.dll' ||
        base === 'ggml.dll' ||
        base === 'ggml-base.dll' ||
        (base.startsWith('ggml-cpu-') && base.endsWith('.dll'))
      ) {
        return base;
      }
      return null;
    }
  );

  // Copy single-file binaries yt-dlp.exe & qjs.exe
  const ytDlpServe = path.join(
    outDir,
    'components',
    'yt-dlp',
    PINNED_COMPONENTS.ytDlp.version,
    'yt-dlp.exe'
  );
  fs.mkdirSync(path.dirname(ytDlpServe), { recursive: true });
  fs.copyFileSync(ytDlpRaw, ytDlpServe);

  const qjsServeV1 = path.join(
    outDir,
    'components',
    'qjs',
    PINNED_COMPONENTS.qjs.version,
    'qjs.exe'
  );
  fs.mkdirSync(path.dirname(qjsServeV1), { recursive: true });
  fs.copyFileSync(qjsRaw, qjsServeV1);

  const componentsV1 = [
    {
      component: 'ffmpeg',
      version: ffmpegVer,
      platform: 'windows-x86_64',
      url: `${baseUrl}/components/ffmpeg/${ffmpegVer}/ffmpeg.zip`,
      mirrors: [],
      size_bytes: ffmpegPkg.size_bytes,
      sha256: ffmpegPkg.sha256,
      archive: 'zip',
      files: ffmpegPkg.files,
      min_app_version: '0.1.0',
      rollout_percent: 100,
    },
    {
      component: 'yt-dlp',
      version: PINNED_COMPONENTS.ytDlp.version,
      platform: 'windows-x86_64',
      url: `${baseUrl}/components/yt-dlp/${PINNED_COMPONENTS.ytDlp.version}/yt-dlp.exe`,
      mirrors: [],
      size_bytes: fs.statSync(ytDlpServe).size,
      sha256: PINNED_COMPONENTS.ytDlp.sha256,
      archive: 'none',
      files: [],
      min_app_version: '0.1.0',
      rollout_percent: 100,
    },
    {
      component: 'qjs',
      version: PINNED_COMPONENTS.qjs.version,
      platform: 'windows-x86_64',
      url: `${baseUrl}/components/qjs/${PINNED_COMPONENTS.qjs.version}/qjs.exe`,
      mirrors: [],
      size_bytes: fs.statSync(qjsServeV1).size,
      sha256: PINNED_COMPONENTS.qjs.sha256,
      archive: 'none',
      files: [],
      min_app_version: '0.1.0',
      rollout_percent: 100,
    },
    {
      component: 'whisper-cli',
      version: whisperVer,
      platform: 'windows-x86_64',
      url: `${baseUrl}/components/whisper-cli/${whisperVer}/whisper-cli.zip`,
      mirrors: [],
      size_bytes: whisperPkg.size_bytes,
      sha256: whisperPkg.sha256,
      archive: 'zip',
      files: whisperPkg.files,
      min_app_version: '0.1.0',
      rollout_percent: 100,
    },
  ];

  if (!skipModels) {
    for (const modelItem of [
      PINNED_COMPONENTS.sileroVad,
      PINNED_COMPONENTS.whisperModelBase,
      PINNED_COMPONENTS.whisperModelSmallQ5_1,
    ]) {
      const mRaw = await ensureVerifiedFile(modelItem, cacheDir);
      const mDest = path.join(outDir, 'models', modelItem.component);
      fs.mkdirSync(path.dirname(mDest), { recursive: true });
      fs.copyFileSync(mRaw, mDest);
      componentsV1.push({
        component: modelItem.component,
        version: modelItem.version,
        platform: 'windows-x86_64',
        url: `${baseUrl}/models/${modelItem.component}`,
        mirrors: [],
        size_bytes: fs.statSync(mDest).size,
        sha256: modelItem.sha256,
        archive: 'none',
        files: [],
        min_app_version: '0.1.0',
        rollout_percent: 100,
      });
    }
  }

  // Manifest #1 (sequence 1): Clean LGPL/MIT/Unlicense real DEV manifest
  const manifestV1 = buildManifest({
    sequence: 1,
    channel: 'dev',
    components: componentsV1,
  });
  writeAndSignManifest(path.join(outDir, 'manifest.json'), manifestV1, privKeyHex, kid);

  // Manifest #2 (sequence 2): Real S1 gyan.dev GPL essentials FFmpeg build for refusal/quarantine test
  if (!skipGpl) {
    const ffmpegGplRaw = await ensureVerifiedFile(PINNED_COMPONENTS.ffmpegGplZip, cacheDir);
    const gplVer = PINNED_COMPONENTS.ffmpegGplZip.version;
    const gplPkg = repackageZipFiltered(
      ffmpegGplRaw,
      path.join(outDir, 'components', 'ffmpeg', gplVer, 'ffmpeg.zip'),
      (entryName) => {
        const idx = entryName.lastIndexOf('/bin/');
        if (idx === -1) return null;
        const base = entryName.slice(idx + 5);
        return base === 'ffmpeg.exe' || base === 'ffprobe.exe' ? base : null;
      }
    );
    const manifestGpl = buildManifest({
      sequence: 2,
      channel: 'dev',
      components: [
        {
          component: 'ffmpeg',
          version: gplVer,
          platform: 'windows-x86_64',
          url: `${baseUrl}/components/ffmpeg/${gplVer}/ffmpeg.zip`,
          mirrors: [],
          size_bytes: gplPkg.size_bytes,
          sha256: gplPkg.sha256,
          archive: 'zip',
          files: gplPkg.files,
          min_app_version: '0.1.0',
          rollout_percent: 100,
        },
        ...componentsV1.filter((c) => c.component !== 'ffmpeg'),
      ],
    });
    writeAndSignManifest(
      path.join(outDir, 'manifest-gpl-ffmpeg.json'),
      manifestGpl,
      privKeyHex,
      kid
    );
  }

  // Manifest #3 (sequence 3): Second valid version (v2 rollback/roll-forward acceptance test)
  const qjsVer2 = `${PINNED_COMPONENTS.qjs.version}-r2`;
  const qjsServeV2 = path.join(outDir, 'components', 'qjs', qjsVer2, 'qjs.exe');
  fs.mkdirSync(path.dirname(qjsServeV2), { recursive: true });
  fs.copyFileSync(qjsRaw, qjsServeV2);

  const ffmpegVer2 = `${ffmpegVer}-r2`;
  const ffmpegPkg2 = repackageZipFiltered(
    ffmpegLgplRaw,
    path.join(outDir, 'components', 'ffmpeg', ffmpegVer2, 'ffmpeg.zip'),
    (entryName) => {
      const idx = entryName.lastIndexOf('/bin/');
      if (idx === -1) return null;
      const base = entryName.slice(idx + 5);
      return ffmpegKeep.has(base) ? base : null;
    }
  );

  const manifestV2 = buildManifest({
    sequence: 3,
    channel: 'dev',
    components: componentsV1.map((c) => {
      if (c.component === 'qjs') {
        return {
          ...c,
          version: qjsVer2,
          url: `${baseUrl}/components/qjs/${qjsVer2}/qjs.exe`,
        };
      }
      if (c.component === 'ffmpeg') {
        return {
          ...c,
          version: ffmpegVer2,
          url: `${baseUrl}/components/ffmpeg/${ffmpegVer2}/ffmpeg.zip`,
          size_bytes: ffmpegPkg2.size_bytes,
          sha256: ffmpegPkg2.sha256,
          files: ffmpegPkg2.files,
        };
      }
      return c;
    }),
  });
  writeAndSignManifest(path.join(outDir, 'manifest-v2.json'), manifestV2, privKeyHex, kid);

  console.log('\nReal component fixtures and signed manifests generated cleanly:');
  console.log(`  1. ${path.join(outDir, 'manifest.json')} (seq=1)`);
  if (!skipGpl) {
    console.log(`  2. ${path.join(outDir, 'manifest-gpl-ffmpeg.json')} (seq=2)`);
  }
  console.log(`  3. ${path.join(outDir, 'manifest-v2.json')} (seq=3)`);
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) ===
    path.resolve(new URL(import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1'))
) {
  const args = process.argv.slice(2);
  const skipModels = args.includes('--skip-models');
  const skipGpl = args.includes('--skip-gpl');
  runFetchPinnedComponents({ skipModels, skipGpl }).catch((err) => {
    console.error('[fetch-pinned-components] ERROR:', err);
    process.exit(1);
  });
}
