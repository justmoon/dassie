import { createActor } from "@dassie/lib-reactive"

import { IlpType } from "../../ilp-connector/ilp-packet-codec"
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

  actor: () =>
    createActor(() => {
      if (process.env["NODE_ENV"] === "production") {
        throw new Error('The "stub" subnet cannot be used in production')
      }
    }),

  processIncomingPacket({ packet, subnetId, balanceMap }) {
    if (packet.type === IlpType.Fulfill) {
      balanceMap.adjustBalance(subnetId, -packet.prepare.amount)
    }
  },

  processOutgoingPacket({ packet, subnetId, balanceMap }) {
    if (packet.type === IlpType.Fulfill) {
      balanceMap.adjustBalance(subnetId, packet.prepare.amount)
    }
  },
} satisfies SubnetModule

export default stub
