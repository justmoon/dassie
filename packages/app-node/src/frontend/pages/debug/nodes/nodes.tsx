import { useRemoteSignal } from "@dassie/lib-reactive-rpc/client"

import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../../../components/ui/table"
import { rpc } from "../../../utils/rpc"

export function Nodes() {
  const nodeTable = useRemoteSignal(rpc.debug.subscribeNodeTable)
  const routingTable = useRemoteSignal(rpc.debug.subscribeRoutingTable)

  const { data: ilpAllocationScheme } =
    rpc.general.getAllocationScheme.useQuery(undefined)

  if (!ilpAllocationScheme || !nodeTable || !routingTable) {
    return null
  }

  const sortedNodeTable = [...nodeTable.values()]
    .map((node) => {
      const routingTableEntry = routingTable.get(
        `${ilpAllocationScheme}.das.${node.nodeId}`,
      )

      if (routingTableEntry?.type !== "peer") {
        return {
          ...node,
          distance: Number.POSITIVE_INFINITY,
          nextHopNodes: [],
        }
      }

      return {
        ...node,
        distance: routingTableEntry.distance,
        nextHopNodes: routingTableEntry.firstHopOptions,
      }
    })
    // Sort first by distance, then by nodeId
    .sort((a, b) => {
      if (a.distance === b.distance) {
        return a.nodeId.localeCompare(b.nodeId)
      }

      return a.distance - b.distance
    })

  return (
    <div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className="w-[100px]">Node</TableHead>
            <TableHead>Distance</TableHead>
            <TableHead>Next Hop</TableHead>
            <TableHead>Neighbors</TableHead>
            <TableHead>Peer State</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {sortedNodeTable.map(
            ({ nodeId, distance, nextHopNodes, linkState, peerState }) => (
              <TableRow key={nodeId}>
                <TableCell>
                  <NodeName
                    nodeId={nodeId}
                    url={linkState?.url ?? "no link state"}
                  />
                </TableCell>
                <TableCell>{distance}</TableCell>
                <TableCell>
                  {nextHopNodes.map((nodeId) => (
                    <NodeName key={nodeId} nodeId={nodeId} />
                  ))}
                </TableCell>
                <TableCell>
                  {linkState?.neighbors.map((nodeId) => (
                    <NodeName key={nodeId} nodeId={nodeId} />
                  ))}
                </TableCell>
                <TableCell>{peerState.id}</TableCell>
              </TableRow>
            ),
          )}
        </TableBody>
      </Table>
    </div>
  )
}

interface NodeNameProperties {
  nodeId: string
  url?: string
}

const NodeName = ({ nodeId, url }: NodeNameProperties) => {
  return (
    <div>
      <p className="font-bold w-32 truncate">{nodeId}</p>
      <p className="text-sm text-muted-foreground">{url}</p>
    </div>
  )
}
