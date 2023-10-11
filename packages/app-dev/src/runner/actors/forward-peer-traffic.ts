import { OutgoingPeerMessageTopic } from "@dassie/app-node/src/backend/peer-protocol/actors/send-peer-message"
import { createActor } from "@dassie/lib-reactive"

import { convertVanityNodeIdToFriendly } from "../../common/utils/vanity-node-id-to-friendly"
import { TrpcClientServiceActor } from "../services/trpc-client"

export const ForwardPeerTrafficActor = () =>
  createActor((sig) => {
    sig.on(OutgoingPeerMessageTopic, async ({ destination }) => {
      const trpcClient = sig.get(TrpcClientServiceActor)
      if (!trpcClient) return

      await trpcClient.runner.notifyPeerTraffic.mutate({
        from: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        to: convertVanityNodeIdToFriendly(destination),
      })
    })
  })
