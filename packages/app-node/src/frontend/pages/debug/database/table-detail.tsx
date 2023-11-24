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
import { trpc } from "../../../utils/trpc"

interface TableDetailParameters {
  id: DatabaseTableId
  name: DatabaseTableName
  columns: readonly string[]
}

export function TableDetail({ id, columns }: TableDetailParameters) {
  const rows = trpc.debug.getDatabaseTableRows.useQuery(id).data ?? []
  return (
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
                {row[column as keyof typeof row]}
              </TableCell>
            ))}
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
