import { Wallet, encodeSeed } from "xrpl"

import { RealmType } from "../../../../config/environment-config"

export const loadOrCreateWallet = <TRealm extends RealmType>(
  seed: Uint8Array,
  realm: TRealm,
): Wallet => {
  if (realm !== "test") {
    throw new Error(`Realm ${realm} is not supported`)
  }

  return Wallet.fromSeed(encodeSeed(seed, "ed25519"))
}
