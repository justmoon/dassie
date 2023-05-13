import { createTopic } from "@dassie/lib-reactive"

import { Transfer } from "../stores/ledger"

export const postedTransfersTopic = () =>
  createTopic<Transfer & { state: "posted" }>()
