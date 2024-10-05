import type { NodeTableEntry } from "@dassie/app-dassie/src/peer-protocol/stores/node-table"
import type { NodeId } from "@dassie/app-dassie/src/peer-protocol/types/node-id"
import type { RoutingInfo } from "@dassie/app-dassie/src/routing/signals/routing-table"

interface NodeDetailProperties {
  nodeId: NodeId
  nodeTableEntry: NodeTableEntry | undefined
  nodeRoute: RoutingInfo | undefined
}

export function NodeDetail({ nodeId, nodeRoute }: NodeDetailProperties) {
  return (
    <div>
      <h2 className="text-3xl font-bold">Details for {nodeId}</h2>
      <div>
        Distance:{" "}
        {(nodeRoute && nodeRoute.type === "peer" && nodeRoute.distance) ??
          "???"}
      </div>
    </div>
  )
}
