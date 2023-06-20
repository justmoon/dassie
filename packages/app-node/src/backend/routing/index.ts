import { createActor } from "@dassie/lib-reactive"

import { calculateRoutes } from "./calculate-routes"

export const doRouting = () =>
  createActor((sig) => {
    sig.run(calculateRoutes)
  })
