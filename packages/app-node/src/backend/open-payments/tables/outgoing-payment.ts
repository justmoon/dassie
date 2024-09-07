import { type InferRow, column, table } from "@dassie/lib-sqlite"

import type { LedgerId } from "../../accounting/constants/ledgers"

export const outgoingPaymentTable = table({
  name: "outgoing_payment",
  columns: {
    id: column().type("TEXT").notNull().primaryKey(),
    destination: column().type("TEXT").notNull(),
    ledger: column().type("TEXT").notNull().typescriptType<LedgerId>(),
    total_amount: column().type("INTEGER").notNull(),
    metadata: column().type("TEXT").notNull(),
  },
})

export type OutgoingPaymentRow = InferRow<typeof outgoingPaymentTable>
