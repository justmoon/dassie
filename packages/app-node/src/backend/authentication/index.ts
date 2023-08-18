import { createActor } from "@dassie/lib-reactive"

import { registerLoginRoute } from "./http-routes/login"

export const startAuthenticationFeature = () =>
  createActor((sig) => {
    sig.run(registerLoginRoute)
  })
