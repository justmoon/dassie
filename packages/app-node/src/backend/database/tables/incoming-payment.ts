import { InferRowReadType, column, table } from "@dassie/lib-sqlite"

export const incomingPaymentTable = table({
  name: "incoming_payment",
  columns: {
    id: column().type("TEXT").required().primaryKey(),
    subnet: column().type("TEXT").required(),
    total_amount: column().type("INTEGER").required(),
    received_amount: column().type("INTEGER").required(),
    external_reference: column().type("TEXT").required(),
  },
})

export type IncomingPaymentRow = InferRowReadType<typeof incomingPaymentTable>
