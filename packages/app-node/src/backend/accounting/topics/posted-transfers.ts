import { createTopic } from "@dassie/lib-reactive"

import { Transfer } from "../stores/ledger"

export const PostedTransfersTopic = () =>
  createTopic<Transfer & { state: "posted" }>()
