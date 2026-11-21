// Jalankan: node serve.mjs   lalu buka http://localhost:8765
// Wallet Retium tidak tersedia di file:// atau alamat http selain localhost.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.dirname(fileURLToPath(import.meta.url));
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.png': 'image/png' };
const port = Number(process.env.PORT || 8765);
const MAX_BYTES = 4 * 1024 * 1024;

async function handleUpload(req, res) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > MAX_BYTES + 1024 * 1024) {
      res.writeHead(413, { 'Content-Type': 'application/json' }).end(JSON.stringify({ error: 'Payload too large' }));
      return;
    }
    chunks.push(chunk);
  }
  const send = (code, obj) => {
    res.writeHead(code, { 'Content-Type': 'application/json' }).end(JSON.stringify(obj));
  };
  try {
    const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (typeof body.data !== 'string' || !body.data) return send(400, { error: 'Missing image data' });
    const bytes = Buffer.from(body.data, 'base64');
    if (!bytes.length) return send(400, { error: 'Empty image' });
    if (bytes.length > MAX_BYTES) return send(413, { error: 'Image exceeds 4 MB' });

    const safeName = String(body.name || 'image').replace(/[^a-zA-Z0-9._-]/g, '_').slice(-80) || 'image';
    const form = new FormData();
    form.append('image', new Blob([bytes], { type: body.mime || 'image/png' }), safeName);
    const up = await fetch('https://basepic.com/api/v1/images', { method: 'POST', body: form });
    if (!up.ok) return send(502, { error: 'Image host returned ' + up.status });
    const info = await up.json();
    if (!info.image_url) return send(502, { error: 'Image host returned no URL' });
    return send(200, { url: info.image_url });
  } catch (e) {
    return send(500, { error: e && e.message ? e.message : 'Upload failed' });
  }
}

http.createServer(async (req, res) => {
  if ((req.url || '').split('?')[0] === '/api/upload') {
    if (req.method !== 'POST') {
      res.writeHead(405, { 'Content-Type': 'application/json' }).end(JSON.stringify({ error: 'Method not allowed' }));
      return;
    }
    await handleUpload(req, res);
    return;
  }

  const rel = decodeURIComponent((req.url || '').split('?')[0]);
  const file = path.join(root, rel === '/' ? 'index.html' : rel);
  if (!file.startsWith(root)) { res.writeHead(403).end(); return; }
  fs.readFile(file, (err, data) => {
    if (err) { res.writeHead(404).end('not found'); return; }
    res.writeHead(200, { 'Content-Type': types[path.extname(file)] || 'application/octet-stream' });
    res.end(data);
  });
}).listen(port, '127.0.0.1', () => console.log(`Imago market: http://localhost:${port}`));
