import { type InferRowType, defineTable } from "@dassie/lib-sqlite"

export const incomingPaymentTable = defineTable({
  name: "incoming_payment",
  columns: [
    { name: "id", type: "TEXT", required: true, primaryKey: true },
    { name: "subnet", type: "TEXT", required: true },
    { name: "total_amount", type: "INTEGER", required: true },
    { name: "received_amount", type: "INTEGER", required: true },
    { name: "external_reference", type: "TEXT", required: true },
  ],
})

export type IncomingPaymentRow = InferRowType<typeof incomingPaymentTable>
