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

  behavior: () => {
    if (process.env["NODE_ENV"] === "production") {
      throw new Error('The "stub" subnet cannot be used in production')
    }

    return {
      settle: ({ amount, peerKey }) => {
        console.log(`Sending settlement for ${amount} units to ${peerKey}`)
      },
    }
  },
} satisfies SubnetModule

export default stub
