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
import { PeerRoutingDetail } from "./detail/peer"

export function Routing() {
  type RoutingTable = inferObservableValue<
    RouterOutput["subscribeRoutingTable"]
  >
  const [routingTable, setRoutingTable] = useState<RoutingTable>()

  trpc.subscribeRoutingTable.useSubscription(undefined, {
    onData: (data) => {
      setRoutingTable(data)
    },
  })

  if (!routingTable) return null

  const sortedRoutingTable = [...routingTable.entries()]
    .map(([prefix, entry]) => {
      return {
        ...entry,
        prefix,
      }
    })
    // Sort first by distance, then by nodeId
    .sort((a, b) => {
      return a.prefix.localeCompare(b.prefix)
    })

  return (
    <div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className="w-[100px]">ILP Prefix</TableHead>
            <TableHead>Type</TableHead>
            <TableHead>Details</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {sortedRoutingTable.map((entry) => (
            <TableRow key={entry.prefix}>
              <TableCell>{entry.prefix}</TableCell>
              <TableCell>{entry.type}</TableCell>
              <TableCell>
                {entry.type === "peer" ? (
                  <PeerRoutingDetail {...entry} />
                ) : null}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
