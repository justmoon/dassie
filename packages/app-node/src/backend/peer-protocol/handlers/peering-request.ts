import type {
  IncomingPeerMessageHandlerParameters,
  PeerMessageContent,
} from "../handle-peer-message"
import { peerTableStore } from "../stores/peer-table"

export const handlePeeringRequest = (
  content: PeerMessageContent<"peeringRequest">,
  { reactor }: IncomingPeerMessageHandlerParameters
) => {
  const { nodeInfo } = content

  const { subnetId, nodeId, url, nodePublicKey } = nodeInfo.signed
  reactor.use(peerTableStore).addPeer({
    subnetId,
    nodeId,
    state: { id: "peered" },
    url,
    nodePublicKey,
    lastSeen: Date.now(),
  })
}
