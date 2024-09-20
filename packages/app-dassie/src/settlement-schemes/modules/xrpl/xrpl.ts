import { Client } from "xrpl"

import { castLedgerId } from "../../../accounting/utils/cast-ledger-id"
import { settlementXrpl as logger } from "../../../logger/instances"
import type { SettlementSchemeModule } from "../../types/settlement-scheme-module"
import { XRP_VALUE_FACTOR } from "./constants/asset-scale"
import { CreateSettlementEngine } from "./functions/create-settlement-engine"
import { getAccountInfo } from "./functions/get-account-info"
import { loadOrCreateWallet } from "./functions/load-wallet"
import type { XrplPeerState } from "./types/peer-state"

const LEDGER_ID = castLedgerId("xrpl+xrp")

/**
 * This module uses the XRP Ledger (mainnet) for settlement.
 *
 * @remarks
 *
 * This module uses real money. USE AT YOUR OWN RISK. This software is
 * provided under the terms of the Apache License 2.0. See the LICENSE file
 * for more information.
 */
const xrpl = {
  name: "xrpl",
  supportedVersions: [1],
  realm: "live",

  ledger: LEDGER_ID,

  behavior: async ({ sig, host }) => {
    const createSettlementEngine = sig.reactor.use(CreateSettlementEngine)
    const seed = host.getEntropy({ path: "seed-mainnet" }).subarray(0, 16)

    const wallet = loadOrCreateWallet(seed)

    const client = new Client("wss://s1.ripple.com/")

    client.on(
      "error",
      (
        errorCode: unknown,
        errorMessage: unknown,
        error: unknown,
        ...otherParameters
      ) => {
        logger.error("xrpl client error", {
          errorCode,
          errorMessage,
          error,
          otherParameters,
        })
      },
    )

    await client.connect()

    sig.onCleanup(async () => {
      await client.disconnect()
    })

    // Ensure account exists and otherwise create it using the testnet faucet.
    const ownAccountInfo = await getAccountInfo(client, wallet.address)
    if (ownAccountInfo) {
      logger.info("xrp account found", {
        address: wallet.address,
        balance: ownAccountInfo.result.account_data.Balance,
      })

      const balance =
        BigInt(ownAccountInfo.result.account_data.Balance) * XRP_VALUE_FACTOR

      host.reportDeposit({ ledgerId: LEDGER_ID, amount: balance })
    }

    return await createSettlementEngine({
      client,
      wallet,
      host,
      ledgerId: LEDGER_ID,
    })
  },
} satisfies SettlementSchemeModule<XrplPeerState>

export default xrpl
