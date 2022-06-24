import type { Subnet } from "../types/subnet"

/**
 * The stub subnet pretends to settle but does not actually do anything.
 *
 * @remarks
 *
 * **WARNING** This module is intended for testing and development. You **must not** use this module in a real node otherwise anyone will be able to take your funds.
 */
const createSubnet: Subnet = () => {
  if (process.env["NODE_ENV"] === "production") {
    throw new Error('The "stub" subnet cannot be used in production')
  }

  return {
    name: "stub",
    supportedVersions: [1],
    realm: "real",

    getAddress() {
      return "stub"
    },

    sendMoney(settlementRequest) {
      return Promise.resolve({
        ...settlementRequest,
        canonicalTransactionId: "stub",
      })
    },

    verifyIncomingTransaction() {
      return Promise.resolve(true)
    },
  }
}

export default createSubnet
