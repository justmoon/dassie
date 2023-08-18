import { createActor } from "@dassie/lib-reactive"

import { registerDevelopmentSetSessionRoute } from "./http-routes/development-set-session"
import { registerLoginRoute } from "./http-routes/login"

export const startAuthenticationFeature = () =>
  createActor((sig) => {
    sig.run(registerLoginRoute)

    if (import.meta.env.DEV) sig.run(registerDevelopmentSetSessionRoute)
  })
