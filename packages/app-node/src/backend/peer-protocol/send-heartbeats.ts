import { createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { PeersSignal } from "./computed/peers"
import {
  MAX_HEARTBEAT_INTERVAL,
  MIN_HEARTBEAT_INTERVAL,
} from "./constants/timings"
import { NodeTableStore } from "./stores/node-table"

export const SendHeartbeatsActor = () =>
  createActor(async (sig: DassieActorContext) => {
    const ownNodeId = sig.read(NodeIdSignal)

    // Get the current peers and re-run the actor if they change
    const peers = sig.readAndTrack(PeersSignal)

    const linkStateUpdate: Uint8Array | undefined = sig.readAndTrack(
      NodeTableStore,
      (nodeTable) => nodeTable.get(ownNodeId),
    )?.linkState?.lastUpdate

    if (!linkStateUpdate) {
      return
    }

    const sendHeartbeat = () => {
      for (const peer of peers) {
        sig.reactor.use(SendPeerMessageActor).api.send.tell({
          destination: peer,
          message: {
            type: "linkStateUpdate",
            value: {
              bytes: linkStateUpdate,
            },
          },
        })
      }
    }

    // Send heartbeats to direct peers
    const task = sig.task({
      handler: sendHeartbeat,
      interval: MIN_HEARTBEAT_INTERVAL,
      jitter: MAX_HEARTBEAT_INTERVAL - MIN_HEARTBEAT_INTERVAL,
    })

    await task.execute()
    task.schedule()
  })
