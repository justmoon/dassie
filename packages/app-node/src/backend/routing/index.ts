import { createActor } from "@dassie/lib-reactive"

import { CalculateRoutesActor } from "./calculate-routes"

export const RoutingActor = () =>
  createActor((sig) => {
    sig.run(CalculateRoutesActor)
  })
