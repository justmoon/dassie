import { createComputed } from "@dassie/lib-reactive"

export const outputPathSignal = () =>
  createComputed(() => {
    return new URL("../../dist", import.meta.url).pathname
  })
