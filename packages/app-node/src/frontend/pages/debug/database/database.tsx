import { useState } from "react"

import { ScrollArea } from "../../../components/ui/scroll-area"
import { combine } from "../../../utils/class-helper"
import { rpc } from "../../../utils/rpc"
import { TableDetail } from "./table-detail"

export function Database() {
  const tables = rpc.debug.getDatabaseTables.useQuery().data ?? []
  const [currentTable, setCurrentTable] = useState<
    (typeof tables)[number] | undefined
  >(undefined)

  return (
    <div className="grid grid-cols-[24rem_auto] h-full min-h-0">
      <ScrollArea>
        <ul className="flex flex-col p-2 pr-4 space-y-2 min-h-0">
          {tables.map((table) => (
            <li
              key={table.id}
              className={combine(
                "rounded-md p-2 cursor-pointer hover:bg-accent",
                table.id === currentTable?.id && "bg-accent",
              )}
              onClick={() => setCurrentTable(table)}
            >
              {table.name}
            </li>
          ))}
        </ul>
      </ScrollArea>
      {currentTable ? (
        <TableDetail
          key={currentTable.id}
          id={currentTable.id}
          name={currentTable.name}
          columns={currentTable.columns}
        />
      ) : null}
    </div>
  )
}
