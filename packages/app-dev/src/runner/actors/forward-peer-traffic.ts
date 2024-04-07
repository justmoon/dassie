import { OutgoingPeerMessageTopic } from "@dassie/app-node/src/backend/peer-protocol/functions/send-peer-message"
import { createActor } from "@dassie/lib-reactive"

import { convertVanityNodeIdToFriendly } from "../../common/utils/vanity-node-id-to-friendly"
import { RpcClientServiceActor } from "../services/rpc-client"

export const ForwardPeerTrafficActor = () =>
  createActor((sig) => {
    sig.on(OutgoingPeerMessageTopic, async ({ destination }) => {
      const rpcClient = sig.readAndTrack(RpcClientServiceActor)
      if (!rpcClient) return

      await rpcClient.notifyPeerTraffic.mutate({
        from: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        to: convertVanityNodeIdToFriendly(destination),
      })
    })
  })
