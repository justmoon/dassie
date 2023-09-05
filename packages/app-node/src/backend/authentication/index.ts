import { createActor } from "@dassie/lib-reactive"

import { registerDevelopmentSetSessionRoute } from "./http-routes/development-set-session"
import { registerLoginRoute } from "./http-routes/login"
import { registerLogoutRoute } from "./http-routes/logout"

export const startAuthenticationFeature = () =>
  createActor((sig) => {
    sig.run(registerLoginRoute)
    sig.run(registerLogoutRoute)

    if (import.meta.env.DEV) sig.run(registerDevelopmentSetSessionRoute)
  })
