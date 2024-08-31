import {
  DatabaseTableId,
  DatabaseTableName,
} from "../../../../backend/database/schema"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../../../components/ui/table"
import { rpc } from "../../../utils/rpc"

interface TableDetailParameters {
  id: DatabaseTableId
  name: DatabaseTableName
  columns: readonly string[]
}

function formatValue(value: unknown) {
  if (typeof value === "bigint") return value.toString()

  return String(value)
}

export function TableDetail({ id, columns }: TableDetailParameters) {
  const rows = rpc.debug.getDatabaseTableRows.useQuery(id).data ?? []
  return (
    <div>
      <Table>
        <TableHeader>
          <TableRow>
            {columns.map((column) => (
              <TableHead key={column}>{column}</TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row, index) => (
            <TableRow key={index}>
              {columns.map((column) => (
                <TableCell key={column}>
                  {formatValue(row[column as keyof typeof row])}
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
