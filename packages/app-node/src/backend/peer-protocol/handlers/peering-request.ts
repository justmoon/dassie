import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type { IncomingPeerMessageEvent } from "../actions/handle-peer-message"
import { peerTableStore } from "../stores/peer-table"

export const handlePeeringRequest = () =>
  createActor((sig) => {
    const peerTable = sig.use(peerTableStore)
    return ({
      message: {
        content: {
          value: { value: content },
        },
      },
    }: IncomingPeerMessageEvent<"peeringRequest">) => {
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

      return EMPTY_UINT8ARRAY
    }
  })
