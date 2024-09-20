import { createTopic } from "@dassie/lib-reactive"

import type { Transfer } from "../stores/ledger"

export const PostedTransfersTopic = () =>
  createTopic<Transfer & { state: "posted" }>()
