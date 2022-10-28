import { InferRowType, defineTable } from "@dassie/lib-sqlite"

export const incomingPaymentTable = defineTable({
  name: "incoming_payment",
  columns: {
    id: {
      type: "string",
    },
    subnet: {
      type: "string",
    },
    total_amount: {
      type: "integer",
    },
    received_amount: {
      type: "integer",
    },
    external_reference: {
      type: "string",
    },
  },
})

export type IncomingPaymentRow = InferRowType<typeof incomingPaymentTable>
