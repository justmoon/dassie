import { Client } from "xrpl"

import { bufferToUint8Array, isFailure } from "@dassie/lib-type-utils"

import { castLedgerId } from "../../../accounting/utils/cast-ledger-id"
import { NodeIdSignal } from "../../../ilp-connector/computed/node-id"
import { settlementXrpl as logger } from "../../../logger/instances"
import type { SettlementSchemeModule } from "../../types/settlement-scheme-module"
import { XRP_SETTLEMENT_MEMO_TYPE } from "./constants/settlement-memo-type"
import { getAccountInfo } from "./functions/get-account-info"
import { IsSettlement } from "./functions/is-settlement"
import { loadOrCreateWallet } from "./functions/load-wallet"
import { peeringInfoSchema } from "./oer-schemas/peering-info-data"
import { peeringRequestSchema } from "./oer-schemas/peering-request-data"
import { peeringResponseSchema } from "./oer-schemas/peering-response-data"
import { settlementProofSchema } from "./oer-schemas/settlement-proof"
import { XrplPeerState } from "./types/peer-state"

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
    const isSettlement = sig.reactor.use(IsSettlement)

    const seed = host.getEntropy({ path: "seed" }).subarray(0, 16)

    const wallet = loadOrCreateWallet(seed, "test")

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
    }

    await client.request({
      command: "subscribe",
      accounts: [wallet.address],
    })

    client.on("transaction", (transaction) => {
      if (transaction.meta?.AffectedNodes) {
        for (const node of transaction.meta.AffectedNodes) {
          // Did this transaction affect our balance?
          if (
            "ModifiedNode" in node &&
            node.ModifiedNode.LedgerEntryType === "AccountRoot" &&
            node.ModifiedNode.FinalFields?.["Account"] === wallet.address &&
            typeof node.ModifiedNode.FinalFields["Balance"] === "string" &&
            typeof node.ModifiedNode.PreviousFields?.["Balance"] === "string" &&
            node.ModifiedNode.FinalFields["Balance"] !==
              node.ModifiedNode.PreviousFields["Balance"]
          ) {
            const newBalance =
              BigInt(node.ModifiedNode.FinalFields["Balance"]) *
              XRP_VALUE_FACTOR
            const oldBalance =
              BigInt(node.ModifiedNode.PreviousFields["Balance"]) *
              XRP_VALUE_FACTOR

            const isSettlementResult = isSettlement(transaction)

            if (newBalance > oldBalance) {
              // If a transaction increased our balance, it's either an incoming settlement or a deposit.
              if (
                isSettlementResult.isSettlement &&
                isSettlementResult.direction === "incoming"
              ) {
                // If it is tagged as a settlement, process it as such
                const amount = newBalance - oldBalance
                host.reportIncomingSettlement({
                  ledgerId: LEDGER_ID,
                  peerId: isSettlementResult.peerId,
                  amount,
                })
              } else {
                // Otherwise it's a deposit.
                const amount = newBalance - oldBalance
                host.reportDeposit({ ledgerId: LEDGER_ID, amount })
              }
            } else {
              // If a transaction decreased our balance, it's either an outgoing settlement or a withdrawal
              if (
                isSettlementResult.isSettlement &&
                isSettlementResult.direction === "outgoing"
              ) {
                logger.assert(
                  !!transaction.transaction.hash,
                  "expected transaction hash to be present",
                )
                host.finalizeOutgoingSettlement({
                  settlementId: transaction.transaction.hash,
                })
              } else {
                const amount = oldBalance - newBalance
                host.reportWithdrawal({ ledgerId: LEDGER_ID, amount })
              }
            }
          }
        }
      }
    })

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
          logger.debug?.("failed to parse peering request data", {
            peer: peerId,
          })
          return false
        }

        const { address } = parseResult.value

        if (!(await getAccountInfo(client, address))) {
          logger.debug?.("peer account not found", {
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
      prepareSettlement: async ({ peerId, amount, peerState }) => {
        logger.info("preparing settlement", { to: peerId, amount })

        const prepared = await client.autofill({
          TransactionType: "Payment" as const,
          Account: wallet.address,
          // Divide by 10^3 because the XRP Ledger uses 3 less decimal places than the internal representation.
          // We also round up to the nearest integer.
          Amount: String((amount + XRP_VALUE_FACTOR - 1n) / XRP_VALUE_FACTOR),
          Destination: peerState.address,
          Memos: [
            {
              Memo: {
                MemoType: XRP_SETTLEMENT_MEMO_TYPE,
                MemoData: Buffer.from(sig.read(NodeIdSignal)).toString("hex"),
              },
            },
          ],
        })

        const signed = wallet.sign(prepared)

        const transactionHash = bufferToUint8Array(
          Buffer.from(signed.hash, "hex"),
        )

        return {
          message: settlementProofSchema.serializeOrThrow({
            transactionHash,
          }),
          settlementId: signed.hash,
          execute: async () => {
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

            return {}
          },
        }
      },
      handleSettlement: ({ peerId, amount }) => {
        logger.info("received settlement claim", { from: peerId, amount })

        // Nothing to do here because we process settlements based on the on-ledger transaction.
      },
      handleMessage: () => {
        // no-op
      },
      handleDeposit: () => {
        throw new Error("not implemented")
      },
    }
  },
} satisfies SettlementSchemeModule<XrplPeerState>

export default xrplTestnet
