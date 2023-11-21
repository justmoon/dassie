import { createUnconstructable } from "@dassie/lib-reactive"

export const RestartCallbackUnconstructable = () =>
  createUnconstructable<() => void>()
