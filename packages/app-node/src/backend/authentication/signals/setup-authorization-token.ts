import { randomBytes } from "node:crypto"

import { createSignal } from "@dassie/lib-reactive"

export const SetupAuthorizationTokenSignal = () =>
  createSignal(randomBytes(32).toString("hex"))
