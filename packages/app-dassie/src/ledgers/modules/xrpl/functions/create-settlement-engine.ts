import type { Client, Wallet } from "xrpl"

import { assert } from "@dassie/lib-logger"
import type { Reactor } from "@dassie/lib-reactive"
import { bufferToUint8Array, isFailure } from "@dassie/lib-type-utils"

import type { LedgerId } from "../../../../accounting/constants/ledgers"
import { NodeIdSignal } from "../../../../ilp-connector/computed/node-id"
import { settlementXrpl as logger } from "../../../../logger/instances"
import type {
  SettlementSchemeActorMethods,
  SettlementSchemeHostMethods,
} from "../../../types/settlement-scheme-module"
import { XRP_VALUE_FACTOR } from "../constants/asset-scale"
import { XRP_SETTLEMENT_MEMO_TYPE } from "../constants/settlement-memo-type"
import { peeringInfoSchema } from "../oer-schemas/peering-info-data"
import { peeringRequestSchema } from "../oer-schemas/peering-request-data"
import { peeringResponseSchema } from "../oer-schemas/peering-response-data"
import { settlementProofSchema } from "../oer-schemas/settlement-proof"
import type { XrplPeerState } from "../types/peer-state"
import { getAccountInfo } from "./get-account-info"
import { IsSettlement } from "./is-settlement"

interface CreateSettlementModuleParameters {
  client: Client
  wallet: Wallet
  host: SettlementSchemeHostMethods
  ledgerId: LedgerId
}

export const CreateSettlementEngine = (reactor: Reactor) => {
  const isSettlement = reactor.use(IsSettlement)
  const nodeIdSignal = reactor.use(NodeIdSignal)

  return async function createSettlementEngine({
    client,
    wallet,
    host,
    ledgerId,
  }: CreateSettlementModuleParameters) {
    await client.request({
      command: "subscribe",
      accounts: [wallet.address],
    })

    const ownAccountInfo = await getAccountInfo(client, wallet.address)

    let balance =
      ownAccountInfo ?
        BigInt(ownAccountInfo.result.account_data.Balance) * XRP_VALUE_FACTOR
      : 0n

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

            balance = newBalance

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
                  ledgerId,
                  peerId: isSettlementResult.peerId,
                  amount,
                })
              } else {
                // Otherwise it's a deposit.
                const amount = newBalance - oldBalance
                host.reportDeposit({ ledgerId, amount })
              }
            } else {
              // If a transaction decreased our balance, it's either an outgoing settlement or a withdrawal
              if (
                isSettlementResult.isSettlement &&
                isSettlementResult.direction === "outgoing"
              ) {
                assert(
                  logger,
                  !!transaction.transaction.hash,
                  "expected transaction hash to be present",
                )
                host.finalizeOutgoingSettlement({
                  settlementId: transaction.transaction.hash,
                })
              } else {
                const amount = oldBalance - newBalance
                host.reportWithdrawal({ ledgerId, amount })
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
                MemoData: Buffer.from(nodeIdSignal.read()).toString("hex"),
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
      getBalance: () => balance,
    } satisfies SettlementSchemeActorMethods<XrplPeerState>
  }
}
