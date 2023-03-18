import type { EffectContext } from "@dassie/lib-reactive"

import type { PeerMessageContent } from "../actions/handle-peer-message"
import { peerTableStore } from "../stores/peer-table"

export const handlePeeringRequest = (sig: EffectContext) => {
  const peerTable = sig.use(peerTableStore)
  return (content: PeerMessageContent<"peeringRequest">) => {
    const { nodeInfo } = content

    const { subnetId, nodeId, url, nodePublicKey } = nodeInfo.signed
    peerTable.addPeer({
      subnetId,
      nodeId,
      state: { id: "peered" },
      url,
      nodePublicKey,
      lastSeen: Date.now(),
    })
  }
}
