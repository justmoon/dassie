import { Client } from "xrpl"

import { isFailure } from "@dassie/lib-type-utils"

import { LedgerId } from "../../../accounting/types/ledger-id"
import { EnvironmentConfigSignal } from "../../../config/environment-config"
import { settlementXrpl as logger } from "../../../logger/instances"
import { bufferToUint8Array } from "../../../utils/buffer-to-typedarray"
import type { SettlementSchemeModule } from "../../types/settlement-scheme-module"
import { getAccountInfo } from "./functions/get-account-info"
import { getTransaction } from "./functions/get-transaction"
import { loadOrCreateWallet } from "./functions/load-wallet"
import { peeringInfoSchema } from "./oer-schemas/peering-info-data"
import { peeringRequestSchema } from "./oer-schemas/peering-request-data"
import { peeringResponseSchema } from "./oer-schemas/peering-response-data"
import { settlementProofSchema } from "./oer-schemas/settlement-proof"
import { XrplPeerState } from "./types/peer-state"

// This is just something used during testing to convert between USD and XRP.
const XRP_VALUE_FACTOR = 100_000n

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

  ledger: {
    id: "xrpl-testnet" as LedgerId,
    currency: {
      code: "XRP",
      scale: 9,
    },
  },

  // eslint-disable-next-line unicorn/consistent-function-scoping
  behavior: async ({ sig }) => {
    // TODO: This should probably be stored in the database instead?
    const { dataPath } = sig.read(EnvironmentConfigSignal)

    const xrplWalletPath = `${dataPath}/xrpl-wallet.json`

    const wallet = await loadOrCreateWallet(xrplWalletPath, "test")

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

    sig.onCleanup(async () => await client.disconnect())

    // Ensure account exists and otherwise create it using the testnet faucet.
    const ownAccountInfo = await getAccountInfo(client, wallet.address)
    if (ownAccountInfo) {
      logger.info("xrp account found", {
        address: wallet.address,
        balance: ownAccountInfo.result.account_data.Balance,
      })
    } else {
      logger.info("account not found, funding account using testnet faucet", {
        address: wallet.address,
      })
      await client.fundWallet(wallet)
    }

    return {
      getPeeringInfo() {
        return {
          data: peeringInfoSchema.serializeOrThrow({
            address: wallet.address,
          }),
        }
      },
      createPeeringRequest: () => {
        return {
          data: peeringRequestSchema.serializeOrThrow({
            address: wallet.address,
          }),
        }
      },
      acceptPeeringRequest: async ({ peerId, data }) => {
        const parseResult = peeringRequestSchema.parse(data)

        if (isFailure(parseResult)) {
          logger.debug("failed to parse peering request data", {
            peer: peerId,
          })
          return false
        }

        const { address } = parseResult.value

        if (!(await getAccountInfo(client, address))) {
          logger.debug("peer account not found", {
            peer: peerId,
            address,
          })
          return false
        }

        return {
          peeringResponseData: peeringResponseSchema.serializeOrThrow(),
          peerState: {
            address,
          },
        }
      },
      finalizePeeringRequest: ({ peeringInfo }) => {
        // If we get here, we have successfully parsed these bytes before, so if parsing fails now, it's a bug, so we
        // just throw.
        const peeringInfoParseResult =
          peeringInfoSchema.parseOrThrow(peeringInfo)

        return {
          peerState: {
            address: peeringInfoParseResult.value.address,
          },
        }
      },
      settle: async ({ peerId, amount, peerState }) => {
        logger.info("preparing settlement", { to: peerId, amount })

        const prepared = await client.autofill({
          TransactionType: "Payment" as const,
          Account: wallet.address,
          Amount: String(1n + amount / XRP_VALUE_FACTOR),
          Destination: peerState.address,
        })

        const signed = wallet.sign(prepared)

        logger.info("submitting settlement transaction", {
          to: peerId,
          amount,
          xrplAmount: prepared.Amount,
          hash: signed.hash,
        })
        const submitResult = await client.submitAndWait(signed.tx_blob)

        logger.info("settlement transaction processed, notifying peer", {
          to: peerId,
          amount,
          submitResult,
        })

        const transactionHash = bufferToUint8Array(
          Buffer.from(signed.hash, "hex"),
        )

        return {
          proof: settlementProofSchema.serializeOrThrow({
            transactionHash,
          }),
        }
      },
      handleSettlement: async ({ peerId, amount, proof }) => {
        logger.info("received settlement claim", { from: peerId, amount })

        const parseResult = settlementProofSchema.parse(proof)

        if (isFailure(parseResult)) {
          logger.warn("failed to parse settlement proof", {
            from: peerId,
            amount,
            error: parseResult,
          })
          return {
            result: "reject",
          }
        }

        const transactionHash = Buffer.from(
          parseResult.value.transactionHash,
        ).toString("hex")

        const transaction = await getTransaction(client, transactionHash)

        if (!transaction) {
          logger.warn("settlement transaction not found", {
            from: peerId,
            amount,
            transactionHash,
          })
          return {
            result: "reject",
          }
        }

        const deliveredAmount = transaction.meta.delivered_amount

        if (typeof deliveredAmount !== "string") {
          logger.warn(
            "settlement transaction delivered amount is undefined or is not XRP",
            {
              from: peerId,
              amount,
              deliveredAmount,
              transactionHash,
            },
          )
          return {
            result: "reject",
          }
        }

        const receivedAmount = BigInt(deliveredAmount) * XRP_VALUE_FACTOR

        if (receivedAmount < amount) {
          logger.warn("settlement delivered amount was lower than claimed", {
            from: peerId,
            amount,
            receivedAmount,
          })
          return {
            result: "reject",
          }
        }

        logger.info("settlement transaction verified", {
          from: peerId,
          amount,
          receivedAmount,
        })

        return {
          result: "accept",
        }
      },
      handleMessage: () => {
        // no-op
      },
    }
  },
} satisfies SettlementSchemeModule<XrplPeerState>

export default xrplTestnet
