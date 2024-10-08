import { createActor } from "@dassie/lib-reactive"

import { RouteLocalEndpointsActor } from "./route-local-endpoints"

export const LocalIlpActor = () =>
  createActor((sig) => {
    sig.run(RouteLocalEndpointsActor)
  })
