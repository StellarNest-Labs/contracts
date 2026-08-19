#!/usr/bin/env bash
# Builds and deploys the StellarNest treasury contract to a Stellar network.
#
# Usage: ./scripts/deploy.sh <network> <source-account>
#   network:        local | testnet | futurenet | mainnet (default: testnet)
#   source-account:  a funded account/key alias known to `stellar keys`
set -euo pipefail

NETWORK="${1:-testnet}"
SOURCE="${2:-treasury-deployer}"

echo "==> Building treasury contract (wasm32v1-none, release, optimized)"
stellar contract build

WASM_PATH="target/wasm32v1-none/release/treasury.wasm"

echo "==> Deploying to $NETWORK as $SOURCE"
CONTRACT_ID=$(stellar contract deploy \
  --wasm "$WASM_PATH" \
  --source "$SOURCE" \
  --network "$NETWORK")

echo "==> Deployed treasury contract: $CONTRACT_ID"
echo "$CONTRACT_ID" > ".contract-id.$NETWORK"
echo "Saved to .contract-id.$NETWORK"
