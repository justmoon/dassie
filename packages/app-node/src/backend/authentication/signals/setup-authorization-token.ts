import { randomBytes } from "node:crypto"

import { createSignal } from "@dassie/lib-reactive"

export const setupAuthorizationTokenSignal = () =>
  createSignal(randomBytes(32).toString("hex"))
