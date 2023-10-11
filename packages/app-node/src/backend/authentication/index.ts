import { createActor } from "@dassie/lib-reactive"

import { HasNodeIdentitySignal } from "../config/computed/has-node-identity"
import { RegisterDevelopmentSetSessionRouteActor } from "./http-routes/development-set-session"
import { RegisterLoginRouteActor } from "./http-routes/login"
import { RegisterLogoutRouteActor } from "./http-routes/logout"
import { RegisterSetupRouteActor } from "./http-routes/setup"

export const AuthenticationFeatureActor = () =>
  createActor((sig) => {
    if (sig.get(HasNodeIdentitySignal)) {
      sig.run(RegisterLoginRouteActor)
      sig.run(RegisterLogoutRouteActor)

      if (import.meta.env.DEV) sig.run(RegisterDevelopmentSetSessionRouteActor)
    } else {
      sig.run(RegisterSetupRouteActor)
    }
  })
