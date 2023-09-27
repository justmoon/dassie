import { createActor } from "@dassie/lib-reactive"

import { hasNodeIdentityComputed } from "../config/computed/has-node-identity"
import { registerDevelopmentSetSessionRoute } from "./http-routes/development-set-session"
import { registerLoginRoute } from "./http-routes/login"
import { registerLogoutRoute } from "./http-routes/logout"
import { registerSetupRoute } from "./http-routes/setup"

export const startAuthenticationFeature = () =>
  createActor((sig) => {
    if (sig.get(hasNodeIdentityComputed)) {
      sig.run(registerLoginRoute)
      sig.run(registerLogoutRoute)

      if (import.meta.env.DEV) sig.run(registerDevelopmentSetSessionRoute)
    } else {
      sig.run(registerSetupRoute)
    }
  })
