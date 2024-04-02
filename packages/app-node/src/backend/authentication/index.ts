import { createActor } from "@dassie/lib-reactive"

import type { DassieActorContext } from "../base/types/dassie-base"
import { HasNodeIdentitySignal } from "../config/computed/has-node-identity"
import { RegisterDevelopmentSetSessionRouteActor } from "./http-routes/development-set-session"
import { RegisterLoginRouteActor } from "./http-routes/login"
import { RegisterLogoutRouteActor } from "./http-routes/logout"
import { RegisterSetupRouteActor } from "./http-routes/setup"

export const AuthenticationFeatureActor = () =>
  createActor((sig: DassieActorContext) => {
    if (sig.readAndTrack(HasNodeIdentitySignal)) {
      sig.run(RegisterLoginRouteActor)
      sig.run(RegisterLogoutRouteActor)

      if (import.meta.env.DEV) sig.run(RegisterDevelopmentSetSessionRouteActor)
    } else {
      sig.run(RegisterSetupRouteActor)
    }
  })
