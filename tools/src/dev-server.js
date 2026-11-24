#!/usr/bin/env node
/**
 * tools/src/dev-server.js
 *
 * Local test server simulating CDN manifest & component artifact distribution.
 * Implements HTTP/1.1 Range (RFC 7233), ETag, and specific error-injection test modes
 * (PROGRESS_AND_ETA §8, SECURITY §9).
 */

import * as fs from 'node:fs';
import * as http from 'node:http';
import * as path from 'node:path';
import * as url from 'node:url';

/**
 * Creates and starts the development / test HTTP server.
 *
 * @param {Object} options
 * @param {number} [options.port=0] 0 for dynamic random port
 * @param {string} options.rootDir Root directory containing static files
 * @returns {Promise<{ server: http.Server, port: number, url: string, close: () => Promise<void> }>}
 */
export function startDevServer({ port = 0, rootDir }) {
  return new Promise((resolve, reject) => {
    const server = http.createServer(async (req, res) => {
      try {
        const parsedUrl = new URL(req.url, `http://${req.headers.host || '127.0.0.1'}`);
        const testMode = parsedUrl.searchParams.get('test_mode');

        // Mode: redirect to another host
        if (testMode === 'redirect') {
          const target =
            parsedUrl.searchParams.get('target') || 'https://malicious-redirect.example.com';
          res.writeHead(302, { Location: target });
          res.end();
          return;
        }

        // Resolve local file path
        const decodedPath = decodeURIComponent(parsedUrl.pathname).replace(/^\/+/, '');
        const targetPath = path.resolve(rootDir, decodedPath);

        // Path containment check inside rootDir
        if (!targetPath.startsWith(path.resolve(rootDir))) {
          res.writeHead(403, { 'Content-Type': 'text/plain' });
          res.end('Forbidden');
          return;
        }

        if (!fs.existsSync(targetPath) || fs.statSync(targetPath).isDirectory()) {
          res.writeHead(404, { 'Content-Type': 'text/plain' });
          res.end('Not Found');
          return;
        }

        const stat = fs.statSync(targetPath);
        let fileBuffer = fs.readFileSync(targetPath);
        const totalSize = fileBuffer.length;
        const etag = `"${stat.size.toString(16)}-${stat.mtimeMs.toString(16)}"`;

        // Mode: wrong_content (tamper/corrupt the payload)
        if (testMode === 'wrong_content') {
          fileBuffer = Buffer.from(fileBuffer);
          if (fileBuffer.length > 0) {
            fileBuffer[0] ^= 0xff; // Flip bits in first byte
          } else {
            fileBuffer = Buffer.from('corrupted');
          }
        }

        // Range header handling
        const rangeHeader = req.headers['range'];
        const ifRange = req.headers['if-range'];

        let isRange = false;
        let start = 0;
        let end = totalSize - 1;

        if (rangeHeader && (!ifRange || ifRange === etag)) {
          const match = rangeHeader.match(/bytes=(\d+)-(\d*)/);
          if (match) {
            isRange = true;
            start = parseInt(match[1], 10);
            if (match[2]) {
              end = parseInt(match[2], 10);
            }
            if (start > end || start >= totalSize) {
              res.writeHead(416, { 'Content-Range': `bytes */${totalSize}` });
              res.end();
              return;
            }
            end = Math.min(end, totalSize - 1);
          }
        }

        const sliceToSend = fileBuffer.subarray(start, end + 1);
        const contentLength = sliceToSend.length;

        const headers = {
          'Accept-Ranges': 'bytes',
          ETag: etag,
          'Content-Length': contentLength,
          'Content-Type': targetPath.endsWith('.json')
            ? 'application/json'
            : 'application/octet-stream',
        };

        if (isRange) {
          headers['Content-Range'] = `bytes ${start}-${end}/${totalSize}`;
          res.writeHead(206, headers);
        } else {
          res.writeHead(200, headers);
        }

        // Mode: oversize body (send more bytes than declared Content-Length)
        if (testMode === 'oversize') {
          res.write(sliceToSend);
          res.write(Buffer.alloc(8192, 0xaa));
          res.end();
          return;
        }

        // Mode: drop connection mid-transfer
        if (testMode === 'drop') {
          const dropAfter = parseInt(parsedUrl.searchParams.get('after') || '256', 10);
          const chunk1 = sliceToSend.subarray(0, Math.min(dropAfter, sliceToSend.length));
          res.write(chunk1);
          // Destroy TCP connection abruptly
          setTimeout(() => {
            req.socket.destroy();
          }, 20);
          return;
        }

        // Mode: throttle delivery
        if (testMode === 'throttle') {
          const chunkSize = parseInt(parsedUrl.searchParams.get('rate') || '1024', 10);
          let offset = 0;
          const interval = setInterval(() => {
            if (offset >= sliceToSend.length) {
              clearInterval(interval);
              res.end();
              return;
            }
            const chunk = sliceToSend.subarray(offset, offset + chunkSize);
            offset += chunkSize;
            res.write(chunk);
          }, 50);
          return;
        }

        // Normal delivery
        res.end(sliceToSend);
      } catch (err) {
        res.writeHead(500, { 'Content-Type': 'text/plain' });
        res.end(String(err));
      }
    });

    server.listen(port, '127.0.0.1', () => {
      const addr = server.address();
      const actualPort = typeof addr === 'object' && addr ? addr.port : port;
      resolve({
        server,
        port: actualPort,
        url: `http://127.0.0.1:${actualPort}`,
        close: () =>
          new Promise((resolveClose) => {
            server.close(() => resolveClose());
          }),
      });
    });

    server.on('error', reject);
  });
}
