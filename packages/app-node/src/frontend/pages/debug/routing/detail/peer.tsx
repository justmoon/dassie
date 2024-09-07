import type { PeerRoutingInfo } from "../../../../../backend/routing/signals/routing-table"

export const PeerRoutingDetail = ({
  firstHopOptions,
  distance,
}: PeerRoutingInfo) => {
  return <span>{`${firstHopOptions.join(", ")} (distance: ${distance})`}</span>
}
