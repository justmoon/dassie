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

    host.reportOnLedgerBalance({ ledgerId: ledger.id, balance })

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
      settle: ({ peerId, amount }) => {
        if (amount > balance) {
          logger.error("Insufficient balance", { amount, balance })
          throw new Error("Insufficient balance")
        }
        logger.info(`Sending settlement for ${amount} units to ${peerId}`)

        balance -= amount

        return {
          proof: new Uint8Array(),
        }
      },
      handleSettlement: ({ peerId, amount }) => {
        logger.info(`Received settlement for ${amount} units from ${peerId}`)

        balance += amount

        return {
          result: "accept" as const,
        }
      },
      handleMessage: () => {
        // no-op
      },
    }
  },
} satisfies SettlementSchemeModule<object>

export default stub
