import { PeerDestinationInfo } from "../../../../../backend/ilp-connector/senders/send-peer-packets"

export const PeerRoutingDetail = ({
  firstHopOptions,
  distance,
}: PeerDestinationInfo) => {
  return <span>{`${firstHopOptions.join(", ")} (distance: ${distance})`}</span>
}
