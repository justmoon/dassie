import { Wallet } from "xrpl"

import { readFile, writeFile } from "node:fs/promises"

import { isErrorWithCode } from "@dassie/lib-type-utils"

import { RealmType } from "../../../../config/environment-config"
import { xrplWalletSchema } from "../zod-schemas/wallet"

export const loadOrCreateWallet = async <TRealm extends RealmType>(
  path: string,
  realm: TRealm,
): Promise<Wallet> => {
  if (realm !== "test") {
    throw new Error(`Realm ${realm} is not supported`)
  }

  let wallet: Wallet | undefined

  try {
    const serializedWalletData = await readFile(path, "utf8")
    const deserializedWalletData = JSON.parse(serializedWalletData) as unknown
    const parsedWalletData = xrplWalletSchema.parse(deserializedWalletData)

    if (parsedWalletData.realm !== realm) {
      throw new Error("Wallet realm does not match")
    }

    wallet = Wallet.fromSeed(parsedWalletData.seed)
  } catch (error) {
    if (isErrorWithCode(error, "ENOENT")) {
      // No wallet found, create a new one
      wallet = Wallet.generate()

      if (!wallet.seed) {
        throw new Error("Failed to generate seed")
      }

      await writeFile(
        path,
        JSON.stringify({
          realm,
          seed: wallet.seed,
          address: wallet.address,
        }),
      )
    } else {
      throw error
    }
  }

  if (!wallet) {
    throw new Error("Failed to load wallet")
  }

  return wallet
}
