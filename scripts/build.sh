#!/usr/bin/env bash
# The release build. Always SBPF v0: Anchor 1.2.1 defaults to v3, which LiteSVM, devnet and
# mainnet do not load. Prints the ELF and IDL hashes.
#
#   bash scripts/build.sh            # one build
#   bash scripts/build.sh --twice    # clean build twice and require identical hashes
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/share/solana/install/active_release/bin:$HOME/.cargo/bin:$PATH"

want_solana="3.0.0"
want_anchor="1.2.1"
have_solana=$(solana --version | awk '{print $2}')
have_anchor=$(anchor --version | awk '{print $2}')
[[ "$have_solana" == "$want_solana" ]] || { echo "solana $have_solana, need $want_solana" >&2; exit 1; }
[[ "$have_anchor" == "$want_anchor" ]] || { echo "anchor $have_anchor, need $want_anchor" >&2; exit 1; }

so=target/deploy/mud_trade_book.so
idl=target/idl/mud_trade_book.json
# The program id is fixed by declare_id! in lib.rs. Anchor writes a throwaway keypair to
# target/deploy when none exists; it does not change the ELF and cannot deploy to the real id.

# Avoid `cargo clean`: it also deletes target/deploy.
clean() {
  rm -rf target/sbpf-solana-solana target/sbpfv3-solana-solana target/release target/idl target/types
}

build() {
  rm -f "$so" "$idl"
  anchor build --arch v0
  [[ -s "$so" && -s "$idl" ]] || { echo "build produced no artifacts" >&2; exit 1; }
  # The ELF is public once deployed. /Users/runner in it is the platform-tools CI build of std.
  if strings "$so" | grep -qiF -e "$HOME" -e "$(id -un)" -e "$PWD"; then
    echo "the ELF embeds the builder's home, user name or checkout path" >&2
    exit 1
  fi
}

hashes() {
  echo "$(shasum -a 256 "$so" | cut -d' ' -f1) $(wc -c <"$so" | tr -d ' ') $(shasum -a 256 "$idl" | cut -d' ' -f1)"
}

if [[ "${1:-}" == "--twice" ]]; then
  clean
  build
  first=$(hashes)
  clean
  build
  second=$(hashes)
  [[ "$first" == "$second" ]] || { echo "builds differ: $first vs $second" >&2; exit 1; }
  echo "reproducible across two clean builds"
else
  build
fi
read -r elf_sha elf_bytes idl_sha <<<"$(hashes)"
echo "ELF sha256 $elf_sha ($elf_bytes bytes)"
echo "IDL sha256 $idl_sha"
