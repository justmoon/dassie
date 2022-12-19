import { createSignal } from "@dassie/lib-reactive"

import type { SubnetModule } from "../types/subnet-module"

/**
 * The stub subnet pretends to settle but does not actually do anything.
 *
 * @remarks
 *
 * **WARNING** This module is intended for testing and development. You **must not** use this module in a real node otherwise anyone will be able to take your funds.
 */
const createSubnet: SubnetModule = () => {
  if (process.env["NODE_ENV"] === "production") {
    throw new Error('The "stub" subnet cannot be used in production')
  }

  const balance = createSignal(0n)

  return {
    name: "stub",
    supportedVersions: [1],
    realm: "test",

    balance,

    prepareIncomingPacket() {
      // no-op
    },

    fulfillIncomingPacket({ amount }) {
      balance.update((balance) => balance + amount)
    },

    rejectIncomingPacket() {
      // no-op
    },

    prepareOutgoingPacket() {
      // no-op
    },

    fulfillOutgoingPacket({ amount }) {
      balance.update((balance) => balance - amount)
    },

    rejectOutgoingPacket() {
      // no-op
    },

    dispose() {
      // no-op
    },
  }
}

export default createSubnet
