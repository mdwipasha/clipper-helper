import * as crypto from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const rootDir = path.resolve(__dirname, '../..');
const fixturesDir = path.join(rootDir, 'tools', 'dev-fixtures');
const devKeysDir = path.join(rootDir, '.dev-keys');

fs.mkdirSync(path.join(fixturesDir, 'models'), { recursive: true });
fs.mkdirSync(devKeysDir, { recursive: true });

// 1. Dev Key
const privKeyPath = path.join(devKeysDir, 'clipper-manifest-dev.priv.hex');
const pubKeyPath = path.join(devKeysDir, 'clipper-manifest-dev.pub.hex');

let privKeyHex;
let pubKeyHex;

if (fs.existsSync(privKeyPath) && fs.existsSync(pubKeyPath)) {
  privKeyHex = fs.readFileSync(privKeyPath, 'utf8').trim();
  pubKeyHex = fs.readFileSync(pubKeyPath, 'utf8').trim();
} else {
  const { privateKey, publicKey } = crypto.generateKeyPairSync('ed25519');
  const privDer = privateKey.export({ type: 'pkcs8', format: 'der' });
  const pubDer = publicKey.export({ type: 'spki', format: 'der' });
  const rawPub32 = pubDer.subarray(pubDer.length - 32);

  privKeyHex = privDer.toString('hex');
  pubKeyHex = rawPub32.toString('hex');

  fs.writeFileSync(privKeyPath, privKeyHex, { mode: 0o600 });
  fs.writeFileSync(pubKeyPath, pubKeyHex);
}

// 2. Create model artifact (64 KB)
const modelPath = path.join(fixturesDir, 'models', 'ggml-tiny.bin');
const modelBuffer = Buffer.alloc(65536);
for (let i = 0; i < 65536; i++) {
  modelBuffer[i] = (i * 31 + 17) & 0xff;
}
fs.writeFileSync(modelPath, modelBuffer);

const modelSha256 = crypto.createHash('sha256').update(modelBuffer).digest('hex');

// 3. Valid Manifest
const manifest = {
  schema: 1,
  sequence: 1,
  generated_at: new Date().toISOString(),
  channel: 'dev',
  components: [
    {
      component: 'ggml-tiny.bin',
      version: '1.0.0',
      platform: 'windows-x86_64',
      url: 'http://127.0.0.1:8080/models/ggml-tiny.bin',
      size_bytes: modelBuffer.length,
      sha256: modelSha256,
      archive: 'none',
      files: [],
      min_app_version: '0.1.0',
      rollout_percent: 100,
    },
  ],
};

const manifestJson = JSON.stringify(manifest, null, 2);
const manifestPath = path.join(fixturesDir, 'manifest.json');
fs.writeFileSync(manifestPath, manifestJson);

// Sign manifest
const privateKey = crypto.createPrivateKey({
  key: Buffer.from(privKeyHex, 'hex'),
  format: 'der',
  type: 'pkcs8',
});
const sig = crypto.sign(null, Buffer.from(manifestJson), privateKey);

const sigObj = {
  kid: 'clipper-manifest-dev',
  signature: sig.toString('hex'),
};
fs.writeFileSync(path.join(fixturesDir, 'manifest.json.sig'), JSON.stringify(sigObj, null, 2));

// 4. Tampered Manifest (valid json but invalid signature / corrupted payload)
const tamperedManifest = {
  ...manifest,
  sequence: 2,
  components: [
    {
      ...manifest.components[0],
      sha256: '0000000000000000000000000000000000000000000000000000000000000000',
    },
  ],
};
const tamperedJson = JSON.stringify(tamperedManifest, null, 2);
fs.writeFileSync(path.join(fixturesDir, 'manifest-tampered.json'), tamperedJson);

// Deliberately wrong signature for tampered manifest
const badSigObj = {
  kid: 'clipper-manifest-dev',
  signature: 'deadbeef'.repeat(16), // Invalid 64-byte signature
};
fs.writeFileSync(
  path.join(fixturesDir, 'manifest-tampered.json.sig'),
  JSON.stringify(badSigObj, null, 2)
);

console.log('Dev fixtures created successfully in:', fixturesDir);
console.log('Model SHA-256:', modelSha256);
console.log('Dev Key KID: clipper-manifest-dev');
console.log('Dev Key Pub:', pubKeyHex);
