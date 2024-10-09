import { createActor, createMapped } from "@dassie/lib-reactive"
import { isFailure, tell } from "@dassie/lib-type-utils"

import type { DassieReactor } from "../base/types/dassie-base"
import { peerProtocol as logger } from "../logger/instances"
import { PeersSignal } from "./computed/peers"
import { QueryUplinkAddress } from "./functions/query-uplink-address"
import { UplinkAddressesStore } from "./stores/uplink-addresses"

export const ReceiveAddressFromUplinkActor = (reactor: DassieReactor) => {
  const queryUplinkAddress = reactor.use(QueryUplinkAddress)
  const uplinkAddressesStore = reactor.use(UplinkAddressesStore)
  return createMapped(reactor, PeersSignal, (peerId) =>
    createActor((sig) => {
      tell(async () => {
        const uplinkAddress = await queryUplinkAddress(peerId)

        // Ignore result if the actor is already disposed
        if (sig.isDisposed) return

        if (isFailure(uplinkAddress)) {
          logger.error("failed to get address from uplink")
          uplinkAddressesStore.act.updateAddress(peerId, undefined)
          return
        }

        uplinkAddressesStore.act.updateAddress(peerId, uplinkAddress)
      })
    }),
  )
}
