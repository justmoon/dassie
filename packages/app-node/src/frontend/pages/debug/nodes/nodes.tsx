import { useRemoteSignal } from "@dassie/lib-reactive-trpc/client"

import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../../../components/ui/table"
import { trpc } from "../../../utils/trpc"

export function Nodes() {
  const config = useRemoteSignal(trpc.subscribeConfig)
  const nodeTable = useRemoteSignal(trpc.subscribeNodeTable)
  const routingTable = useRemoteSignal(trpc.subscribeRoutingTable)

  const { data: ilpAllocationScheme } =
    trpc.getAllocationScheme.useQuery(undefined)

  if (!ilpAllocationScheme || !config || !nodeTable || !routingTable) {
    return null
  }

  const sortedNodeTable = [...nodeTable.values()]
    .map((node) => {
      const routingTableEntry = routingTable.get(
        `${ilpAllocationScheme}.das.${node.nodeId}`
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
          </TableRow>
        </TableHeader>
        <TableBody>
          {sortedNodeTable.map(
            ({ nodeId, alias, distance, nextHopNodes, linkState }) => (
              <TableRow key={nodeId}>
                <TableCell>
                  <NodeName nodeId={nodeId} alias={alias} />
                </TableCell>
                <TableCell>{distance}</TableCell>
                <TableCell>
                  {nextHopNodes.map((nodeId) => (
                    <NodeName key={nodeId} nodeId={nodeId} />
                  ))}
                </TableCell>
                <TableCell>
                  {linkState.neighbors.map((nodeId) => (
                    <NodeName key={nodeId} nodeId={nodeId} />
                  ))}
                </TableCell>
              </TableRow>
            )
          )}
        </TableBody>
      </Table>
    </div>
  )
}

interface NodeNameProperties {
  nodeId: string
  alias?: string
}

const NodeName = ({ nodeId, alias }: NodeNameProperties) => {
  return (
    <div>
      <p className="font-bold w-32 truncate">{nodeId}</p>
      <p className="text-sm text-muted-foreground">{alias}</p>
    </div>
  )
}
