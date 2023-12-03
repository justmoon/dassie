import { createTopic } from "@dassie/lib-reactive"

import { Transfer } from "../stores/ledger"

export const VoidedTransfersTopic = () =>
  createTopic<Transfer & { state: "voided" }>()
