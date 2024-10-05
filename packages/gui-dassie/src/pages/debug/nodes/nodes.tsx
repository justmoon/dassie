import { useMemo } from "react"

import { NodeTableStore } from "@dassie/app-dassie/src/peer-protocol/stores/node-table"
import {
  useRemoteSignal,
  useRemoteStore,
} from "@dassie/lib-reactive-rpc/client"

import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../../../components/ui/table"
import { rpc } from "../../../utils/rpc"
import { sortNodes } from "../../../utils/sort-nodes"

export function Nodes() {
  const nodeTable = useRemoteStore(
    rpc.network.subscribeToNodeTableStore,
    NodeTableStore,
  )
  const routingTable = useRemoteSignal(rpc.debug.subscribeRoutingTable)

  const { data: basicState } = rpc.general.getBasicState.useQuery()
  const ilpAllocationScheme = basicState?.ilpAllocationScheme

  const sortedNodeTable = useMemo(() => {
    if (!ilpAllocationScheme || !routingTable) return undefined

    return sortNodes(ilpAllocationScheme, nodeTable, routingTable)
  }, [ilpAllocationScheme, nodeTable, routingTable])

  if (!ilpAllocationScheme || !routingTable || !sortedNodeTable) {
    return null
  }

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
