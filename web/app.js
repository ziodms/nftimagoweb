/* Imago NFT Market — Retium testnet frontend.
 * Reads come from the free public RPC. Writes go through the injected Retium
 * wallet: the page never sees a key and the user approves every transaction. */
(() => {
  'use strict';

  const CONTRACT = '5f897b54a8d7b2e3aa41752f15146285623528c01a217842c60afbc60631142f';
  const RPC = 'https://rpc.retium.org';
  const PAGE = 20;
  const IMAGE_HOST = 'https://basepic.com/api/v1/images';

  const $ = (id) => document.getElementById(id);
  const connectBtn = $('connectBtn');
  const mintBtn = $('mintBtn');
  const balanceEl = $('balance');
  const modal = $('modal');
  const modalBody = $('modalBody');
  const toastEl = $('toast');

  const state = {
    address: null,
    chainId: null,
    busy: false,
    tokens: {},
    supply: null,
    marketTokens: [],
    marketNext: '',
    mineTokens: [],
    mineNext: '',
    toastTimer: null,
  };

  const upload = { uri: '', hash: '', file: null, objectUrl: null };

  /* ---------- helpers ---------- */

  function esc(v) {
    return String(v ?? '').replace(/[&<>"']/g, (c) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
    })[c]);
  }

  function short(v, head = 10, tail = 6) {
    const s = String(v ?? '');
    return s.length <= head + tail ? s : s.slice(0, head) + '…' + s.slice(-tail);
  }

  function parseJSON(v) {
    if (typeof v !== 'string') return v;
    try { return JSON.parse(v); } catch { return v; }
  }

  function toAtomic(s) {
    const text = String(s ?? '').trim();
    if (!/^\d+(\.\d{0,18})?$/.test(text)) throw new Error('Invalid RTM amount.');
    const [whole, frac = ''] = text.split('.');
    return (BigInt(whole) * 10n ** 18n + BigInt((frac + '0'.repeat(18)).slice(0, 18))).toString();
  }

  function fmtRtm(atomic, maxFrac = 6) {
    const s = String(atomic ?? '0').padStart(19, '0');
    const whole = s.slice(0, -18).replace(/^0+(?=\d)/, '');
    const frac = s.slice(-18).replace(/0+$/, '').slice(0, maxFrac);
    return frac ? `${whole}.${frac}` : whole;
  }

  function formatBytes(n) {
    if (n < 1024) return n + ' B';
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
    return (n / 1024 / 1024).toFixed(1) + ' MB';
  }

  function imageUrl(uri) {
    if (!uri) return '';
    if (uri.startsWith('https://')) return uri;
    if (uri.startsWith('ipfs://')) return 'https://ipfs.io/ipfs/' + uri.slice(7);
    if (uri.startsWith('ar://')) return 'https://arweave.net/' + uri.slice(5);
    return '';
  }

  function toast(message, kind = 'info') {
    clearTimeout(state.toastTimer);
    toastEl.textContent = message;
    toastEl.className = 'toast ' + kind;
    toastEl.hidden = false;
    state.toastTimer = setTimeout(() => { toastEl.hidden = true; }, 6000);
  }

  async function copyText(text, label = 'Copied') {
    try {
      await navigator.clipboard.writeText(text);
      toast(`${label} to clipboard.`, 'ok');
      return;
    } catch { /* fall through to the legacy copy */ }
    const area = document.createElement('textarea');
    area.value = text;
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    try { document.execCommand('copy'); toast(`${label} to clipboard.`, 'ok'); }
    catch { toast('Could not copy automatically.', 'error'); }
    area.remove();
  }

  async function waitForRetium(timeoutMs = 400) {
    if (window.retium) return window.retium;
    return new Promise((resolve) => {
      const t = setTimeout(() => resolve(window.retium ?? null), timeoutMs);
      window.addEventListener('retium#initialized', () => { clearTimeout(t); resolve(window.retium); }, { once: true });
    });
  }

  /* ---------- chain reads and writes ---------- */

  async function query(method, args = []) {
    let url = `${RPC}/api/contract/${CONTRACT}/query?method=${encodeURIComponent(method)}`;
    if (args.length) url += '&args=' + args.map((a) => encodeURIComponent(String(a))).join(',');
    const res = await fetch(url);
    if (!res.ok) throw new Error(`RPC error ${res.status}`);
    const reply = await res.json();
    if (!reply.ok) throw new Error(reply.error || 'Query failed');
    const inner = parseJSON(reply.output);
    if (inner && inner.ok === false) throw new Error(inner.error || 'Contract rejected the query');
    return inner && typeof inner === 'object' ? inner.data : inner;
  }

  async function token(id) {
    if (state.tokens[id]) return state.tokens[id];
    const t = parseJSON(await query('get_token', [String(id)]));
    if (!t || typeof t !== 'object' || !t.id) throw new Error('Token not found');
    state.tokens[id] = t;
    return t;
  }

  async function waitFinal(txId) {
    for (let i = 0; i < 120; i++) {
      const res = await fetch(`${RPC}/api/transaction/${txId}`);
      if (!res.ok) throw new Error('Could not check transaction status');
      const tx = await res.json();
      const fin = tx.finality || tx.status;
      if (fin === 'HardFinal') {
        if (tx.exec_status && tx.exec_status !== 'ok') throw new Error(tx.exec_error || `Transaction ${tx.exec_status}`);
        return tx;
      }
      if (fin === 'Rejected' || fin === 'Voided') throw new Error(`Transaction ${fin.toLowerCase()}`);
      await new Promise((r) => setTimeout(r, 1000));
    }
    throw new Error('Transaction is still pending after 2 minutes — check the explorer: ' + short(txId, 14, 6));
  }

  async function sendCall(method, args, amountAtomic = '0', label = 'Transaction') {
    if (!state.address) throw new Error('Connect your wallet first.');
    const retium = await waitForRetium();
    if (!retium) throw new Error('Wallet not found. Install it at wallet.retium.org.');
    toast('Approve the transaction in your wallet…', 'info');
    const { txId } = await retium.request({
      method: 'retium_call',
      params: { contractId: CONTRACT, method, args: args.map(String), amountAtomic: String(amountAtomic) },
    });
    toast(`Transaction submitted ${short(txId, 14, 6)} — waiting for finality…`, 'info');
    await waitFinal(txId);
    toast(`${label} successful`, 'ok');
    return txId;
  }

  async function runAction(btn, fn, doRefresh = true) {
    if (state.busy) return;
    state.busy = true;
    syncButtons();
    const oldText = btn.textContent;
    btn.textContent = 'Processing…';
    try {
      await fn();
      if (doRefresh) await refreshAll();
    } catch (e) {
      if (e && e.code === 4001) toast('Rejected in wallet.', 'info');
      else toast(e && e.message ? e.message : String(e), 'error');
    } finally {
      state.busy = false;
      btn.textContent = oldText;
      updateWalletUI();
    }
  }

  /* ---------- wallet ---------- */

  async function connect() {
    const retium = await waitForRetium();
    if (!retium) {
      toast('Wallet not installed — opening wallet.retium.org', 'info');
      window.open('https://wallet.retium.org', '_blank', 'noopener');
      return;
    }
    try {
      const res = await retium.request({ method: 'retium_requestAccounts' });
      state.address = res.address || null;
      state.chainId = res.chainId ?? null;
      if (state.address) localStorage.setItem('imago.connected', '1');
      updateWalletUI();
      if (state.address) { updateBalance(); loadMine(true); }
      toast('Wallet connected: ' + short(state.address), 'ok');
    } catch (e) {
      if (e && e.code === 4001) return;
      toast(e && e.message ? e.message : 'Could not connect the wallet', 'error');
    }
  }

  function disconnect() {
    state.address = null;
    state.chainId = null;
    state.mineTokens = [];
    state.mineNext = '';
    localStorage.removeItem('imago.connected');
    balanceEl.hidden = true;
    balanceEl.textContent = '';
    $('mineGrid').innerHTML = '';
    $('mineEmpty').hidden = true;
    $('mineMore').hidden = true;
    updateWalletUI();
    toast('Wallet disconnected.', 'info');
  }

  async function updateBalance() {
    if (!state.address) return;
    try {
      const res = await fetch(`${RPC}/api/wallet/${state.address}`);
      if (!res.ok) return;
      const info = await res.json();
      const value = Number(info.balance_rtm || 0).toLocaleString('en-US', { maximumFractionDigits: 4 });
      balanceEl.textContent = `${value} RTM`;
      balanceEl.hidden = false;
    } catch { /* balance is optional */ }
  }

  function updateWalletUI() {
    if (state.address) {
      connectBtn.textContent = short(state.address);
      connectBtn.title = state.address + ' — click to disconnect';
      connectBtn.classList.add('connected');
      $('walletNotice').hidden = true;
    } else {
      connectBtn.textContent = 'Connect Wallet';
      connectBtn.title = '';
      connectBtn.classList.remove('connected');
      $('walletNotice').hidden = false;
    }
    updateMintUI();
    syncButtons();
  }

  function syncButtons() {
    document.querySelectorAll('button').forEach((b) => {
      if (b === mintBtn) return; // mint button has its own validation
      b.disabled = state.busy;
    });
    updateMintUI();
  }

  /* ---------- mint: upload, hash, publish ---------- */

  function currentAsset() {
    if (!$('manualFields').hidden) {
      return { uri: $('manualUri').value.trim(), hash: $('manualHash').value.trim() };
    }
    return { uri: upload.uri, hash: upload.hash };
  }

  function setUploadStatus(text, kind) {
    const el = $('uploadStatus');
    el.textContent = text;
    el.className = kind === 'done' ? 'status-done' : kind === 'error' ? 'status-error' : 'status-loading';
  }

  function setStep(n) {
    for (let i = 1; i <= 3; i++) {
      const el = $('step' + i);
      el.classList.toggle('active', i === n);
      el.classList.toggle('done', i < n);
    }
  }

  function updateMintUI() {
    const asset = currentAsset();
    const validAsset = /^(https:\/\/|ipfs:\/\/|ar:\/\/)\S+$/.test(asset.uri)
      && asset.uri.length <= 256
      && /^[0-9a-f]{64}$/.test(asset.hash);
    const hasName = $('tokenName').value.trim().length > 0;
    mintBtn.disabled = state.busy || !state.address || !validAsset || !hasName;
    mintBtn.textContent = state.address ? 'Mint NFT' : 'Connect Wallet to Mint';
    setStep(validAsset ? (hasName ? 3 : 2) : 1);
    $('mintHint').textContent = !state.address
      ? 'Connect your wallet and upload an image to continue.'
      : !validAsset
        ? 'Upload an image or provide a valid URL and SHA-256 hash.'
        : !hasName ? 'Add a name for your token.' : 'Ready — signing happens in your wallet.';
  }

  async function sha256File(file) {
    const bytes = await file.arrayBuffer();
    const digest = await crypto.subtle.digest('SHA-256', bytes);
    return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
  }

  async function uploadImage(file) {
    const safeName = (file.name || 'image').replace(/[^a-zA-Z0-9._-]/g, '_').slice(-80) || 'image';
    const form = new FormData();
    form.append('image', file, safeName);
    const res = await fetch(IMAGE_HOST, { method: 'POST', body: form });
    if (!res.ok) throw new Error('host returned ' + res.status);
    const data = await res.json();
    if (!data.image_url) throw new Error('no URL in host response');
    return data.image_url;
  }

  async function handleFile(file) {
    if (!file) return;
    if (!file.type.startsWith('image/')) { toast('Please choose an image file.', 'error'); return; }
    if (file.size > 16 * 1024 * 1024) { toast('Image must be under 16 MB.', 'error'); return; }

    upload.file = file;
    upload.uri = '';
    upload.hash = '';
    if (upload.objectUrl) URL.revokeObjectURL(upload.objectUrl);
    upload.objectUrl = URL.createObjectURL(file);
    $('previewImg').src = upload.objectUrl;
    $('previewName').textContent = file.name;
    $('previewSize').textContent = formatBytes(file.size);
    $('previewUri').textContent = '—';
    $('previewHash').textContent = '—';
    $('preview').hidden = false;
    $('dropzone').hidden = true;
    $('retryUpload').hidden = true;
    setUploadStatus('Computing SHA-256…', 'loading');
    updateMintUI();

    try {
      upload.hash = await sha256File(file);
      $('previewHash').textContent = short(upload.hash, 16, 10);
      $('previewHash').title = upload.hash;
      setUploadStatus('Uploading image…', 'loading');
      upload.uri = await uploadImage(file);
      $('previewUri').textContent = short(upload.uri, 30, 12);
      $('previewUri').title = upload.uri;
      $('previewUri').style.cursor = 'pointer';
      setUploadStatus('Ready to mint', 'done');
    } catch (e) {
      upload.uri = '';
      upload.hash = '';
      setUploadStatus('Upload failed', 'error');
      $('retryUpload').hidden = false;
      toast('Upload failed: ' + (e.message || e), 'error');
    }
    updateMintUI();
  }

  function resetUpload() {
    if (upload.objectUrl) URL.revokeObjectURL(upload.objectUrl);
    upload.objectUrl = null;
    upload.file = null;
    upload.uri = '';
    upload.hash = '';
    $('fileInput').value = '';
    $('preview').hidden = true;
    $('dropzone').hidden = false;
    $('manualUri').value = '';
    $('manualHash').value = '';
    updateMintUI();
  }

  /* ---------- rendering ---------- */

  function hydrateImages(root) {
    root.querySelectorAll('[data-uri]').forEach((box) => {
      const img = box.querySelector('img.card-img');
      const fallback = box.querySelector('.nft-fallback');
      const url = imageUrl(box.dataset.uri);
      if (!img || !fallback) return;
      if (!url) { img.style.display = 'none'; fallback.style.display = 'flex'; return; }
      img.addEventListener('error', () => { img.style.display = 'none'; fallback.style.display = 'flex'; });
      img.src = url;
    });
  }

  function cardHtml(t) {
    const listed = !!t.is_listed;
    return `
      <article class="card nft-card" data-id="${esc(t.id)}" tabindex="0">
        <div class="nft-media" data-uri="${esc(t.image_uri)}">
          <span class="nft-id">#${esc(t.id)}</span>
          <img class="card-img" alt="">
          <div class="nft-fallback"><img class="fallback-img" src="assets/imagologo.png" alt=""></div>
        </div>
        <div class="nft-body">
          <h3 class="nft-name" title="${esc(t.name)}">${esc(t.name)}</h3>
          <div class="nft-row"><span>Owner</span><span class="mono" title="${esc(t.owner)}">${esc(short(t.owner))}</span></div>
          <div class="nft-price">
            <b>${listed ? fmtRtm(t.price) + ' RTM' : 'Not for sale'}</b>
            <span class="badge ${listed ? 'listed' : 'unlisted'}">${listed ? 'Listed' : 'Unlisted'}</span>
          </div>
        </div>
      </article>`;
  }

  function renderList(containerId, tokens, emptyId, moreBtn, next) {
    const container = $(containerId);
    container.innerHTML = tokens.map(cardHtml).join('');
    hydrateImages(container);
    $(emptyId).hidden = tokens.length > 0;
    moreBtn.hidden = !next;
  }

  async function loadStats() {
    try {
      const [supply, cfg, listed] = await Promise.all([
        query('total_supply'),
        query('get_config').then(parseJSON),
        query('listed_tokens', ['', String(PAGE)]),
      ]);
      state.supply = String(supply);
      $('statSupply').textContent = state.supply;
      $('statListed').textContent = (listed.tokens || []).length + (listed.next ? '+' : '');
      $('statVersion').textContent = cfg && cfg.version ? cfg.version : '—';
    } catch {
      $('statSupply').textContent = '—';
      $('statVersion').textContent = '—';
    }
  }

  async function loadMarket(reset = false) {
    if (reset) { state.marketTokens = []; state.marketNext = ''; }
    try {
      const data = parseJSON(await query('listed_tokens', [state.marketNext, String(PAGE)]));
      state.marketNext = data.next || '';
      for (const id of data.tokens || []) {
        if (!state.marketTokens.some((t) => t.id === id)) state.marketTokens.push({ id });
      }
      state.marketTokens = (await Promise.all(state.marketTokens.map((t) => token(t.id).catch(() => null)))).filter(Boolean);
      renderList('marketGrid', state.marketTokens, 'marketEmpty', $('marketMore'), state.marketNext);
    } catch (e) {
      toast('Could not load marketplace: ' + e.message, 'error');
    }
  }

  async function loadMine(reset = false) {
    if (!state.address) return;
    if (reset) { state.mineTokens = []; state.mineNext = ''; }
    try {
      const data = parseJSON(await query('tokens_by_owner', [state.address, state.mineNext, String(PAGE)]));
      state.mineNext = data.next || '';
      const ids = data.tokens || [];
      const fresh = await Promise.all(ids.map((id) => token(id).catch(() => null)));
      state.mineTokens.push(...fresh.filter(Boolean));
      renderList('mineGrid', state.mineTokens, 'mineEmpty', $('mineMore'), state.mineNext);
    } catch (e) {
      toast('Could not load your NFTs: ' + e.message, 'error');
    }
  }

  async function refreshAll() {
    state.tokens = {};
    await Promise.all([loadStats(), loadMarket(true), loadMine(true), updateBalance()]);
  }

  /* ---------- token detail ---------- */

  function actionHtml(t) {
    const listed = !!t.is_listed;
    const mine = state.address && t.owner === state.address;
    if (!state.address) {
      return `<div class="action-box"><h3>Trade</h3>
        <button class="btn btn-primary btn-block" data-action="connect">Connect Wallet to trade</button></div>`;
    }
    if (mine) {
      return `<div class="action-box">
        <h3>Manage your token</h3>
        <div class="field">
          <label for="listPrice">Sale price (RTM)</label>
          <input id="listPrice" inputmode="decimal" value="${listed ? esc(fmtRtm(t.price, 18)) : ''}" placeholder="0.5">
        </div>
        <div class="action-row">
          <button class="btn btn-primary" data-action="list">${listed ? 'Update price' : 'List for sale'}</button>
          ${listed ? '<button class="btn btn-ghost" data-action="unlist">Unlist</button>' : ''}
        </div>
        <div class="field" style="margin-top:16px">
          <label for="transferTo">Transfer to address</label>
          <input id="transferTo" class="mono" placeholder="rete…" autocomplete="off">
          <small>Double-check the address — the wallet validates its checksum.</small>
        </div>
        <button class="btn btn-danger" data-action="transfer">Transfer NFT</button>
      </div>`;
    }
    if (listed) {
      return `<div class="action-box">
        <h3>Buy this token</h3>
        <p class="price-note">Seller price: <b>${esc(fmtRtm(t.price))} RTM</b> (${esc(t.price)} atomic units).</p>
        <div class="field">
          <label for="maxPrice">Maximum price you will pay (RTM)</label>
          <input id="maxPrice" inputmode="decimal" value="${esc(fmtRtm(t.price, 18))}">
        </div>
        <button class="btn btn-primary btn-block" data-action="buy">Buy for ${esc(fmtRtm(t.price))} RTM</button>
      </div>`;
    }
    return '<div class="action-box"><p class="price-note">This token is not for sale.</p></div>';
  }

  function renderModal(t) {
    modal.dataset.id = String(t.id);
    const created = Number(t.created_at || 0);
    const copyRow = (label, shown, full) => `
      <dt>${label}</dt>
      <dd><span class="mono" title="${esc(full)}">${esc(shown)}</span><button class="copy-inline" data-copy="${esc(full)}" title="Copy ${label}">Copy</button></dd>`;
    modalBody.innerHTML = `
      <div class="modal-layout">
        <div class="modal-media" data-uri="${esc(t.image_uri)}">
          <img class="card-img" alt="">
          <div class="nft-fallback"><img class="fallback-img" src="assets/imagologo.png" alt=""></div>
        </div>
        <div>
          <h2>#${esc(t.id)} — ${esc(t.name)}</h2>
          <div class="sub">Imago NFT · Retium testnet</div>
          <dl class="kv">
            ${copyRow('Contract', short(CONTRACT, 12, 8), CONTRACT)}
            <dt>Supply</dt><dd><span>#${esc(t.id)} of ${esc(state.supply ?? '—')}</span></dd>
            ${copyRow('Owner', short(t.owner, 16, 10), t.owner)}
            ${copyRow('Creator', short(t.creator, 16, 10), t.creator)}
            ${copyRow('Image URI', short(t.image_uri, 36, 12), t.image_uri)}
            ${copyRow('SHA-256', short(t.image_hash, 18, 10), t.image_hash)}
            <dt>Created</dt><dd><span>${created ? esc(new Date(created).toLocaleString('en-US')) : '—'}</span></dd>
            <dt>Status</dt><dd><span>${t.is_listed ? 'Listed · ' + esc(fmtRtm(t.price)) + ' RTM' : 'Not for sale'}</span></dd>
          </dl>
          ${actionHtml(t)}
        </div>
      </div>`;
    hydrateImages(modalBody);
    modal.hidden = false;
    syncButtons();
  }

  async function openToken(id) {
    try {
      if (state.supply === null) state.supply = String(await query('total_supply'));
      renderModal(await token(id));
    } catch (e) { toast(e.message, 'error'); }
  }

  function closeModal() { modal.hidden = true; }

  /* ---------- events ---------- */

  function activateTab(name) {
    document.querySelectorAll('.tab').forEach((x) => x.classList.toggle('active', x.dataset.tab === name));
    document.querySelectorAll('.panel').forEach((p) => p.classList.toggle('active', p.id === 'panel-' + name));
  }

  document.querySelectorAll('.tab').forEach((tab) => {
    tab.addEventListener('click', () => activateTab(tab.dataset.tab));
  });

  connectBtn.addEventListener('click', () => state.address ? disconnect() : runAction(connectBtn, connect, false));

  $('copyContract').addEventListener('click', () => copyText(CONTRACT, 'Contract address copied'));

  $('refreshMarket').addEventListener('click', () => runAction($('refreshMarket'), () => loadMarket(true), false));
  $('refreshMine').addEventListener('click', () => runAction($('refreshMine'), () => loadMine(true), false));
  $('marketMore').addEventListener('click', () => runAction($('marketMore'), () => loadMarket(false), false));
  $('mineMore').addEventListener('click', () => runAction($('mineMore'), () => loadMine(false), false));

  ['marketGrid', 'mineGrid'].forEach((id) => {
    const grid = $(id);
    grid.addEventListener('click', (e) => {
      const card = e.target.closest('.nft-card');
      if (card) openToken(card.dataset.id);
    });
    grid.addEventListener('keydown', (e) => {
      if (e.key !== 'Enter') return;
      const card = e.target.closest('.nft-card');
      if (card) openToken(card.dataset.id);
    });
  });

  $('modalClose').addEventListener('click', closeModal);
  modal.addEventListener('click', (e) => { if (e.target === modal) closeModal(); });
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !modal.hidden) closeModal(); });

  modalBody.addEventListener('click', async (e) => {
    const copyBtn = e.target.closest('[data-copy]');
    if (copyBtn) { e.stopPropagation(); copyText(copyBtn.dataset.copy, 'Copied'); return; }
    const btn = e.target.closest('[data-action]');
    if (!btn) return;
    const action = btn.dataset.action;
    if (action === 'connect') { closeModal(); runAction(connectBtn, connect, false); return; }
    const t = state.tokens[modal.dataset.id];
    if (!t) return;

    if (action === 'list') {
      const price = $('listPrice').value.trim();
      await runAction(btn, () => sendCall('list', [t.id, toAtomic(price)], '0', `Token #${t.id} listed`));
    } else if (action === 'unlist') {
      await runAction(btn, () => sendCall('unlist', [t.id], '0', `Token #${t.id} unlisted`));
    } else if (action === 'transfer') {
      const to = $('transferTo').value.trim();
      if (!/^rete[0-9a-f]{68}$/.test(to)) { toast('Address must be rete + 68 hex characters.', 'error'); return; }
      await runAction(btn, () => sendCall('transfer', [t.id, to], '0', `Token #${t.id} transferred`));
    } else if (action === 'buy') {
      const maxAtomic = toAtomic($('maxPrice').value.trim());
      if (BigInt(maxAtomic) < BigInt(t.price)) { toast('Maximum price is below the listing price.', 'error'); return; }
      await runAction(btn, () => sendCall('buy', [t.id, maxAtomic], t.price, `Token #${t.id} purchased`));
    }
  });

  /* ---------- mint events ---------- */

  const dropzone = $('dropzone');
  dropzone.addEventListener('click', () => $('fileInput').click());
  dropzone.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); $('fileInput').click(); }
  });
  dropzone.addEventListener('dragover', (e) => { e.preventDefault(); dropzone.classList.add('dragover'); });
  dropzone.addEventListener('dragleave', () => dropzone.classList.remove('dragover'));
  dropzone.addEventListener('drop', (e) => {
    e.preventDefault();
    dropzone.classList.remove('dragover');
    handleFile(e.dataTransfer.files[0]);
  });
  $('fileInput').addEventListener('change', (e) => handleFile(e.target.files[0]));
  $('changeImage').addEventListener('click', resetUpload);
  $('retryUpload').addEventListener('click', () => handleFile(upload.file));
  $('previewUri').addEventListener('click', () => {
    if (upload.uri) window.open(upload.uri, '_blank', 'noopener');
  });

  $('manualToggle').addEventListener('click', () => {
    const show = $('manualFields').hidden;
    $('manualFields').hidden = !show;
    $('dropzone').hidden = show;
    $('preview').hidden = show || !upload.file;
    updateMintUI();
  });
  $('manualUri').addEventListener('input', updateMintUI);
  $('manualHash').addEventListener('input', updateMintUI);
  $('tokenName').addEventListener('input', updateMintUI);

  mintBtn.addEventListener('click', () => runAction(mintBtn, async () => {
    const asset = currentAsset();
    const name = $('tokenName').value.trim();
    if (!state.address) throw new Error('Connect your wallet first.');
    if (!name || new TextEncoder().encode(name).length > 48) throw new Error('Name is required and must be at most 48 bytes.');
    if (!/^(https:\/\/|ipfs:\/\/|ar:\/\/)\S+$/.test(asset.uri) || asset.uri.length > 256) {
      throw new Error('Image URL must start with https://, ipfs:// or ar:// and be under 256 bytes.');
    }
    if (!/^[0-9a-f]{64}$/.test(asset.hash)) throw new Error('SHA-256 must be 64 lowercase hex characters.');
    await sendCall('mint', [name, asset.uri, asset.hash], '0', 'NFT minted');
    const supply = await query('total_supply');
    $('tokenName').value = '';
    resetUpload();
    toast(`Mint successful — token #${supply}`, 'ok');
  }));

  /* ---------- boot ---------- */

  $('footerContract').textContent = short(CONTRACT, 10, 10);
  $('statContract').textContent = short(CONTRACT, 8, 6);
  $('mintContract').textContent = short(CONTRACT, 10, 8);
  const initialTab = location.hash.replace('#', '');
  if (['market', 'mine', 'mint'].includes(initialTab)) activateTab(initialTab);
  updateWalletUI();
  loadStats();
  loadMarket(true);

  if (localStorage.getItem('imago.connected') === '1') {
    (async () => {
      try {
        const retium = await waitForRetium(600);
        if (!retium) return;
        const res = await retium.request({ method: 'retium_requestAccounts' });
        if (res && res.address) {
          state.address = res.address;
          state.chainId = res.chainId ?? null;
          updateWalletUI();
          updateBalance();
          loadMine(true);
        } else {
          localStorage.removeItem('imago.connected');
        }
      } catch { /* reconnect silently */ }
    })();
  }
})();
