import { Client } from "xrpl"

import { castLedgerId } from "../../../accounting/utils/cast-ledger-id"
import { settlementXrpl as logger } from "../../../logger/instances"
import type { SettlementSchemeModule } from "../../types/settlement-scheme-module"
import { CreateSettlementEngine } from "./functions/create-settlement-engine"
import { getAccountInfo } from "./functions/get-account-info"
import { loadOrCreateWallet } from "./functions/load-wallet"
import type { XrplPeerState } from "./types/peer-state"

const XRP_ON_LEDGER_SCALE = 6
const XRP_INTERNAL_SCALE = 9
const XRP_VALUE_FACTOR = 10n ** BigInt(XRP_INTERNAL_SCALE - XRP_ON_LEDGER_SCALE)

const LEDGER_ID = castLedgerId("xrpl-testnet+xrp")

/**
 * This module uses the XRP Ledger testnet (altnet) for settlement.
 *
 * @remarks
 *
 * **WARNING** This module is intended for testing and development. You **must not** use this module in a real node otherwise anyone will be able to take your funds.
 */
const xrplTestnet = {
  name: "xrpl-testnet",
  supportedVersions: [1],
  realm: "test",

  ledger: LEDGER_ID,

  behavior: async ({ sig, host }) => {
    const createSettlementEngine = sig.reactor.use(CreateSettlementEngine)

    const seed = host.getEntropy({ path: "seed-testnet" }).subarray(0, 16)

    const wallet = loadOrCreateWallet(seed)

    const client = new Client("wss://s.altnet.rippletest.net:51233")

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
    } else {
      logger.info("account not found, funding account using testnet faucet", {
        address: wallet.address,
      })
      await client.fundWallet(wallet)

      const ownAccountInfo = await getAccountInfo(client, wallet.address)

      if (!ownAccountInfo) {
        throw new Error("Account not found after funding")
      }

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

export default xrplTestnet
