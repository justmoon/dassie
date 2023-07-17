import type { SettlementSchemeModule } from "../types/settlement-scheme-module"

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

  behavior: () => {
    if (process.env["NODE_ENV"] === "production") {
      throw new Error(
        'The "stub" settlement scheme cannot be used in production'
      )
    }

    return {
      settle: ({ peerId, amount }) => {
        console.info(`Sending settlement for ${amount} units to ${peerId}`)
        return {
          proof: new Uint8Array(),
        }
      },
      handleSettlement: ({ peerId, amount }) => {
        console.info(`Received settlement for ${amount} units from ${peerId}`)
        return {
          result: "accept",
        }
      },
      handleMessage: () => {
        // no-op
      },
    }
  },
} satisfies SettlementSchemeModule

export default stub