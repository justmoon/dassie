import { InferRowReadType, column, table } from "@dassie/lib-sqlite"

export const incomingPaymentTable = table({
  name: "incoming_payment",
  columns: {
    id: column().type("TEXT").notNull().primaryKey(),
    subnet: column().type("TEXT").notNull(),
    total_amount: column().type("INTEGER").notNull(),
    received_amount: column().type("INTEGER").notNull(),
    external_reference: column().type("TEXT").notNull(),
  },
})

export type IncomingPaymentRow = InferRowReadType<typeof incomingPaymentTable>
