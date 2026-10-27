# Mud Trade Book

A Solana program for escrowless card-for-card swaps of Mudwater Cards (Metaplex Core
assets, collection `2Qa62ZQoytj3TL4BtRpj8Eujta3zpG93gqXtBg7C5g3p`).

A maker lists the cards they give and the cards they want, up to four on each side. Listed
cards never leave the maker's wallet: the program holds a Core `TransferDelegate` on each one.
A taker fills the offer with matching cards and both sides move in one transaction. Cancelling
or closing an offer revokes every delegation it set.

Program id: `Mudav3fe4qFihgcfCdZAXxNFupTuJR2pUa6Qo7M3CBN`

## Build

Requires Anchor 1.2.1, Solana CLI 3.0.0 and Rust 1.89.0 (`rust-toolchain.toml`).

The tests run in LiteSVM against the real Metaplex Core binary, and the IDL build compiles
them, so dump Core from mainnet before the first build:

```bash
solana program dump -u mainnet-beta CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d tests/fixtures/mpl_core.so
# sha256 51d41542456639a01820c747e72b9d3b4ca258930a5bbb7e78db0cda133dc209 at the time of writing
bash scripts/build.sh          # SBPF v0 release build, prints ELF and IDL hashes
bash scripts/build.sh --twice  # two clean builds, requires identical hashes
```

Expected output for this commit:

```
ELF sha256 d8752bb456ab0b6147cf222efc7aba77ec56d0cea0c8ee27d28cc0e2008eeb4c (454248 bytes)
IDL sha256 d60505c63666a1d6b7b7397d319d1cae03c2ca91067f817edc140a1089607c63
```

## Test

```bash
cargo test
MTB_FUZZ_CASES=20000 cargo test --test fuzz -- --nocapture   # the long fuzz run
```

`tests/fixtures/` holds a real Mudwater card and its collection, dumped from mainnet. The card's
owner field is replaced with a dummy key.
