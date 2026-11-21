# Imago

Imago is an NFT mint-and-market contract on the **Retium testnet**, with a static web frontend.

- Contract: `5f897b54a8d7b2e3aa41752f15146285623528c01a217842c60afbc60631142f`
- Code hash: `25707512d3700967aa8b7eda4a019f74152eb73dad0293fb7a5efaa4447f649f`
- Explorer: https://retium.net/contract/5f897b54a8d7b2e3aa41752f15146285623528c01a217842c60afbc60631142f

Testnet coins have no value.

## Contents

| Path | Description |
|---|---|
| `lib.rs` | Retium contract: `mint`, `transfer`, `list`, `unlist`, `buy`, `get_token`, `tokens_by_owner`, `listed_tokens`, `total_supply`, `get_config` |
| `Cargo.toml`, `Cargo.lock` | Standalone Rust crate for the contract |
| `web/` | English-language frontend with wallet connect, image upload, SHA-256 hashing, minting and marketplace |
| `logo/` | IMAGO logo asset |
| `DEPLOY.md` | Deployment record and verification instructions |

> The contract source is still a draft (`v0.1.0`) with RCP-NFT metadata TODOs. Review it before production use.

## Build the contract

```bash
cd imago
cargo build --release --locked --target wasm32-unknown-unknown
retium verify 5f897b54a8d7b2e3aa41752f15146285623528c01a217842c60afbc60631142f \
  target/wasm32-unknown-unknown/release/imago.wasm
```

## Run the frontend

```bash
cd imago/web
node serve.mjs
```

Open `http://localhost:8765`. The Retium wallet provider is available only on
HTTPS and `localhost`. See `web/README.md` for details.

## Security

Never commit key files, seeds or API keys. See `.gitignore`.
