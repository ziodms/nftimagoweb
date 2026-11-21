// Vercel serverless function: receives the image as base64 JSON from the page
// and forwards it to the image host. Same-origin request, so no CORS issues.
const MAX_BYTES = 4 * 1024 * 1024;

module.exports = async function handler(req, res) {
  res.setHeader('Access-Control-Allow-Origin', '*');
  if (req.method !== 'POST') {
    res.status(405).json({ error: 'Method not allowed' });
    return;
  }

  try {
    const body = typeof req.body === 'string' ? JSON.parse(req.body) : req.body;
    const { name, mime, data } = body || {};
    if (typeof data !== 'string' || !data) {
      res.status(400).json({ error: 'Missing image data' });
      return;
    }

    const bytes = Buffer.from(data, 'base64');
    if (!bytes.length) {
      res.status(400).json({ error: 'Empty image' });
      return;
    }
    if (bytes.length > MAX_BYTES) {
      res.status(413).json({ error: 'Image exceeds 4 MB' });
      return;
    }

    const safeName = String(name || 'image').replace(/[^a-zA-Z0-9._-]/g, '_').slice(-80) || 'image';
    const form = new FormData();
    form.append('image', new Blob([bytes], { type: mime || 'image/png' }), safeName);

    const up = await fetch('https://basepic.com/api/v1/images', { method: 'POST', body: form });
    if (!up.ok) {
      res.status(502).json({ error: 'Image host returned ' + up.status });
      return;
    }
    const info = await up.json();
    if (!info.image_url) {
      res.status(502).json({ error: 'Image host returned no URL' });
      return;
    }
    res.status(200).json({ url: info.image_url });
  } catch (e) {
    res.status(500).json({ error: e && e.message ? e.message : 'Upload failed' });
  }
};
