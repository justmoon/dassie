import { randomBytes } from "node:crypto"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { LedgerId } from "../../accounting/types/ledger-id"
import { settlementStub as logger } from "../../logger/instances"
import type { SettlementSchemeModule } from "../types/settlement-scheme-module"

const INITIAL_BALANCE = 100_000_000_000n

const ledger = {
  id: "stub" as LedgerId,
  currency: {
    code: "USD",
    scale: 9,
  },
}

/**
 * The stub settlement scheme pretends to settle but does not actually do anything.
 *
 * @remarks
 *
 * **WARNING** This module is intended for testing and development. You **must not** use this module in a real node otherwise anyone will be able to take your funds.
 */
const stub = {
  name: "stub",
  supportedVersions: [1],
  realm: "test",

  ledger,

  // eslint-disable-next-line unicorn/consistent-function-scoping
  behavior: ({ host }) => {
    let balance = INITIAL_BALANCE

    host.reportDeposit({ ledgerId: ledger.id, amount: balance })

    return {
      getPeeringInfo: () => {
        return {
          data: EMPTY_UINT8ARRAY,
        }
      },
      createPeeringRequest: () => {
        return {
          data: EMPTY_UINT8ARRAY,
        }
      },
      acceptPeeringRequest: () => {
        return {
          peeringResponseData: EMPTY_UINT8ARRAY,
          peerState: {},
        }
      },
      finalizePeeringRequest: () => {
        return {
          peerState: {},
        }
      },
      prepareSettlement: ({ peerId, amount }) => {
        logger.info("preparing settlement", { to: peerId, amount })
        const settlementId = randomBytes(16).toString("hex")
        return {
          amount,
          message: new Uint8Array(),
          settlementId,

          execute: () => {
            if (amount > balance) {
              logger.error("Insufficient balance", { amount, balance })
              throw new Error("Insufficient balance")
            }
            logger.info("sending settlement", { to: peerId, amount })

            balance -= amount

            host.finalizeOutgoingSettlement({ settlementId })

            return {}
          },
        }
      },
      handleSettlement: ({ peerId, amount }) => {
        logger.info("received settlement", { from: peerId, amount })

        balance += amount

        host.reportIncomingSettlement({
          ledgerId: ledger.id,
          peerId,
          amount,
        })
      },
      handleMessage: () => {
        // no-op
      },
      handleDeposit: ({ amount }) => {
        logger.info(`Received deposit for ${amount} units`)

        balance += amount
        host.reportDeposit({ ledgerId: ledger.id, amount })
      },
    }
  },
} satisfies SettlementSchemeModule<object>

export default stub
