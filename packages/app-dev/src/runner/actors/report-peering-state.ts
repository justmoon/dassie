import { PeersSignal } from "@dassie/app-node/src/backend/peer-protocol/computed/peers"
import { createActor } from "@dassie/lib-reactive"

import { convertVanityNodeIdToFriendly } from "../../common/utils/vanity-node-id-to-friendly"
import { TrpcClientServiceActor } from "../services/trpc-client"

export const ReportPeeringStateActor = () =>
  createActor(async (sig) => {
    const peers = sig.get(PeersSignal)

    const trpcClient = sig.get(TrpcClientServiceActor)
    if (!trpcClient) return

    await trpcClient.runner.notifyPeerState.mutate({
      nodeId: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
      peers: [...peers].map((peer) => convertVanityNodeIdToFriendly(peer)),
    })
  })
