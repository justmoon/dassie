import { type InferRowType, defineTable } from "@dassie/lib-sqlite"

export const incomingPaymentTable = defineTable({
  name: "incoming_payment",
  columns: {
    id: {
      type: "TEXT",
    },
    subnet: {
      type: "TEXT",
    },
    total_amount: {
      type: "INTEGER",
    },
    received_amount: {
      type: "INTEGER",
    },
    external_reference: {
      type: "TEXT",
    },
  },
})

export type IncomingPaymentRow = InferRowType<typeof incomingPaymentTable>
