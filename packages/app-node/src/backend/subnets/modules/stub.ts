import type { SubnetModule } from "../types/subnet-module"

/**
 * The stub subnet pretends to settle but does not actually do anything.
 *
 * @remarks
 *
 * **WARNING** This module is intended for testing and development. You **must not** use this module in a real node otherwise anyone will be able to take your funds.
 */
const stub = {
  name: "stub",
  supportedVersions: [1],
  realm: "test",

  behavior: (_sig, { host }) => {
    if (process.env["NODE_ENV"] === "production") {
      throw new Error('The "stub" subnet cannot be used in production')
    }

    const encoder = new TextEncoder()
    const decoder = new TextDecoder()

    return {
      settle: async ({ peerId, amount }) => {
        console.log(`Sending settlement for ${amount} units to ${peerId}`)
        await host.sendMessage({
          peerId: peerId,
          message: encoder.encode(amount.toString()),
        })
      },
      handleMessage: ({ peerId, message }) => {
        const amount = BigInt(decoder.decode(message))
        console.log(`Received settlement for ${amount} units from ${peerId}`)
      },
    }
  },
} satisfies SubnetModule

export default stub
