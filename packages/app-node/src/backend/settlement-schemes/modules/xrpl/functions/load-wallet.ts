import { Wallet, encodeSeed } from "xrpl"

export const loadOrCreateWallet = (seed: Uint8Array): Wallet => {
  return Wallet.fromSeed(encodeSeed(seed, "ed25519"))
}
