import { createSignal } from "@dassie/lib-reactive"

import type { scenarios } from "../scenarios"

export const ScenarioSignal = () =>
  createSignal<keyof typeof scenarios>("six-nodes")
