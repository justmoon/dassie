import { createTopic } from "@dassie/lib-reactive"

import type { Transfer } from "../stores/ledger"

export const PendingTransfersTopic = () =>
  createTopic<Transfer & { state: "pending" }>()
