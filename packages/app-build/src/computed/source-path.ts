import { createComputed } from "@dassie/lib-reactive"

export const sourcePathSignal = () =>
  createComputed(() => {
    return new URL("../../../app-node", import.meta.url).pathname
  })
