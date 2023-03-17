import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { configSignal } from "../../config"
import type {
  IncomingPeerMessageHandlerParameters,
  PeerMessageContent,
} from "../handle-peer-message"
import { nodeTableStore } from "../stores/node-table"

export const handleLinkStateRequest = (
  { subnetId }: PeerMessageContent<"linkStateRequest">,
  { reactor }: IncomingPeerMessageHandlerParameters
) => {
  const { nodeId } = reactor.use(configSignal).read()
  const ownNodeTableEntry = reactor
    .use(nodeTableStore)
    .read()
    .get(`${subnetId}.${nodeId}`)

  if (!ownNodeTableEntry?.lastLinkStateUpdate) return EMPTY_UINT8ARRAY

  return ownNodeTableEntry.lastLinkStateUpdate
}
