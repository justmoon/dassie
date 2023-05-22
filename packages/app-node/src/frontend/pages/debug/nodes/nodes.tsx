import { inferObservableValue } from "@trpc/server/observable"
import { useState } from "react"

import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../../../components/ui/table"
import { RouterOutput, trpc } from "../../../utils/trpc"

export function Nodes() {
  type NodeTable = inferObservableValue<RouterOutput["subscribeNodeTable"]>
  type RoutingTable = inferObservableValue<
    RouterOutput["subscribeRoutingTable"]
  >
  const [nodeTable, setNodeTable] = useState<NodeTable>()
  const [routingTable, setRoutingTable] = useState<RoutingTable>()

  trpc.subscribeNodeTable.useSubscription(undefined, {
    onData: (data) => {
      setNodeTable(data)
    },
  })
  trpc.subscribeRoutingTable.useSubscription(undefined, {
    onData: (data) => {
      setRoutingTable(data)
    },
  })

  if (!nodeTable || !routingTable) return null

  const sortedNodeTable = [...nodeTable.values()]
    .map((node) => {
      const routingTableEntry = routingTable.get(node.nodeId)

      return {
        ...node,
        distance: routingTableEntry?.distance ?? Number.POSITIVE_INFINITY,
        nextHopNodes: routingTable.get(node.nodeId)?.firstHopOptions ?? [],
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
