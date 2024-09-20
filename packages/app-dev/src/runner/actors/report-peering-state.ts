import { PeersSignal } from "@dassie/app-dassie/src/peer-protocol/computed/peers"
import { createActor } from "@dassie/lib-reactive"

import { convertVanityNodeIdToFriendly } from "../../utils/vanity-node-id-to-friendly"
import type { RpcReactor } from "../services/rpc-client"

export const ReportPeeringStateActor = (reactor: RpcReactor) =>
  createActor(async (sig) => {
    const peers = sig.readAndTrack(PeersSignal)

    await reactor.base.rpc.notifyPeerState.mutate({
      nodeId: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
      peers: [...peers].map((peer) => convertVanityNodeIdToFriendly(peer)),
    })
  })
