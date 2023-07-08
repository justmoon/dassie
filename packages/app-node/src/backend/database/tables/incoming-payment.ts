import { type InferRowType, defineTable } from "@dassie/lib-sqlite"

export const incomingPaymentTable = defineTable({
  name: "incoming_payment",
  columns: [
    { name: "id", type: "TEXT" },
    { name: "subnet", type: "TEXT" },
    { name: "total_amount", type: "INTEGER" },
    { name: "received_amount", type: "INTEGER" },
    { name: "external_reference", type: "TEXT" },
  ],
})

export type IncomingPaymentRow = InferRowType<typeof incomingPaymentTable>
