import type { SetOptional } from "type-fest"

import type { EffectContext } from "@dassie/lib-reactive"

import { databaseService } from "../open-database"

export interface IncomingPaymentRow {
  id: string
  subnet: string
  total_amount: bigint
  received_amount: bigint
  external_reference: string
}

export type NewIncomingPayment = SetOptional<
  IncomingPaymentRow,
  "subnet" | "received_amount"
>

export class IncomingPayment {
  static create(sig: EffectContext, incomingPayment: NewIncomingPayment) {
    const database = sig.get(databaseService)

    if (!database) throw new Error("Database not available")

    const result = database
      .prepare(
        `INSERT INTO incoming_payment (id, subnet, total_amount, received_amount, external_reference) VALUES (@id, @subnet, @total_amount, 0, @external_reference)`
      )
      .run({
        subnet: "null",
        ...incomingPayment,
      })

    return result.lastInsertRowid
  }

  static get(sig: EffectContext, rowId: bigint) {
    const database = sig.get(databaseService)

    if (!database) throw new Error("Database not available")

    const result = database
      .prepare(`SELECT * FROM incoming_payment WHERE rowid = ?`)
      .get(rowId)

    if (!result) return undefined

    return new IncomingPayment(result as IncomingPaymentRow)
  }

  static getAll(sig: EffectContext) {
    const database = sig.get(databaseService)

    if (!database) throw new Error("Database not available")

    const result = database.prepare(`SELECT * FROM incoming_payment`).all()

    return result.map((row) => new IncomingPayment(row as IncomingPaymentRow))
  }

  constructor(public readonly data: IncomingPaymentRow) {}
}
