# Imago NFT Market — web frontend

English-language frontend for the Imago NFT contract on the Retium testnet:
`5f897b54a8d7b2e3aa41752f15146285623528c01a217842c60afbc60631142f`.

## Run locally

```bash
cd imago/web
node serve.mjs
```

Open **http://localhost:8765**. Do not open `index.html` directly (`file://`):
the Retium wallet provider is only available on HTTPS and `localhost`.
Deep links: `/#market`, `/#mine`, `/#mint`.

## Mint flow (upload only)

1. Drop an image (PNG/JPG/WEBP/GIF, max 4 MB) into the upload area or click to browse.
2. The browser computes the **SHA-256** fingerprint locally, then sends the image
   to the same-origin `/api/upload` endpoint. On Vercel this is a serverless
   function (`api/upload.js`); locally it is handled by `serve.mjs`. It forwards
   the file to a free no-key host and returns a public `https://` URL. Same-origin
   means the upload works from any deployed domain, not just localhost.
3. The URL and hash are filled in automatically. Add a name and click **Mint NFT**.
4. The wallet signs the `mint` call; the page waits for `HardFinal`.

A manual URL/hash entry is available through *"Have an image URL already?"* for
images hosted on IPFS (`ipfs://`) or Arweave (`ar://`).

> The contract stores the URL and SHA-256 on-chain, not the image bytes. A free
> host is fine for testnet demos; for production, pin the image to IPFS first and
> use the manual `ipfs://` entry.

## Features

* Connect / disconnect Retium wallet (`retium_requestAccounts`).
* Marketplace: `listed_tokens` with paging, token detail, and `buy` (RTM attached,
  `max_price` editable).
* My NFTs: `tokens_by_owner`, `list`, `unlist`, `transfer`.
* Stats: `total_supply`, listing count, `get_config`.
* BigInt for all amounts. Every write is approved in the wallet and checked for
  `HardFinal` + `exec_status === "ok"`.

## Files

`index.html`, `styles.css`, `app.js`, `serve.mjs`, `assets/imagologo.png`.
The logo file is actually a WebP with a white background despite its `.png`
extension, so the dark theme renders it with `filter: invert(1)` +
`mix-blend-mode: screen`.
