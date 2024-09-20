import type { PeerRoutingInfo } from "@dassie/app-dassie/src/routing/signals/routing-table"

export const PeerRoutingDetail = ({
  firstHopOptions,
  distance,
}: PeerRoutingInfo) => {
  return <span>{`${firstHopOptions.join(", ")} (distance: ${distance})`}</span>
}
