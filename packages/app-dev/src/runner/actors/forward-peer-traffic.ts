import { OutgoingPeerMessageTopic } from "@dassie/app-dassie/src/peer-protocol/functions/send-peer-message"
import { createActor } from "@dassie/lib-reactive"

import { convertVanityNodeIdToFriendly } from "../../utils/vanity-node-id-to-friendly"
import { type RpcReactor } from "../services/rpc-client"

export const ForwardPeerTrafficActor = (reactor: RpcReactor) =>
  createActor((sig) => {
    sig.on(OutgoingPeerMessageTopic, async ({ destination }) => {
      await reactor.base.rpc.notifyPeerTraffic.mutate({
        from: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        to: convertVanityNodeIdToFriendly(destination),
      })
    })
  })
