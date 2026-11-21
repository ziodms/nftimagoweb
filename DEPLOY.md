# Imago NFT Mint & Market — deployment

Contract id : 5f897b54a8d7b2e3aa41752f15146285623528c01a217842c60afbc60631142f
Code hash   : 25707512d3700967aa8b7eda4a019f74152eb73dad0293fb7a5efaa4447f649f (BLAKE3 of the deployed wasm)
Build       : rustc 1.91.1, `cargo build --release --target wasm32-unknown-unknown` in `imago/`

The explorer's "Source not published" notice is expected: Retium stores only the
compiled wasm and its hash, never the source. Anyone with this source can check
that the deployed code is exactly this build:

```bash
cd imago
cargo build --release --locked --target wasm32-unknown-unknown
retium verify 5f897b54a8d7b2e3aa41752f15146285623528c01a217842c60afbc60631142f \
  target/wasm32-unknown-unknown/release/imago.wasm
# -> match
```

The deployed wasm was built on Windows without path remapping. For a build other
people can reproduce byte-for-byte, build inside WSL on Linux:

```bash
RUSTFLAGS="--remap-path-prefix=$PWD=/src --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo" \
  cargo build --release --locked --target wasm32-unknown-unknown
```

That reproducible build produces a different code hash, so it needs its own
deploy to become the contract recorded on chain.
