import { uint8ArrayToHex } from "uint8array-extras"

import { castLedgerId } from "../../accounting/utils/cast-ledger-id"
import { EMPTY_UINT8ARRAY } from "../../constants/general"
import { settlementStub as logger } from "../../logger/instances"
import type { SettlementSchemeModule } from "../types/settlement-scheme-module"

const INITIAL_BALANCE = 100_000_000_000n

const LEDGER_ID = castLedgerId("stub+usd")

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

  ledger: LEDGER_ID,

  behavior: ({ sig, host }) => {
    const { crypto } = sig.reactor.base

    let balance = INITIAL_BALANCE

    host.reportDeposit({ ledgerId: LEDGER_ID, amount: balance })

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
        const settlementId = uint8ArrayToHex(crypto.getRandomBytes(16))
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
          ledgerId: LEDGER_ID,
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
        host.reportDeposit({ ledgerId: LEDGER_ID, amount })
      },
      getBalance: () => balance,
    }
  },
} satisfies SettlementSchemeModule

export default stub
