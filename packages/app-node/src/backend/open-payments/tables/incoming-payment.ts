import { type InferRow, column, table } from "@dassie/lib-sqlite"

export const incomingPaymentTable = table({
  name: "incoming_payment",
  columns: {
    id: column().type("TEXT").notNull().primaryKey(),
    ledger: column().type("TEXT").notNull(),
    total_amount: column().type("INTEGER").notNull(),
    metadata: column().type("TEXT").notNull(),
  },
})

export type IncomingPaymentRow = InferRow<typeof incomingPaymentTable>
