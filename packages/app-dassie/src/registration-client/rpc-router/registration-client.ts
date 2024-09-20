import { subscribeToSignal } from "@dassie/lib-reactive-rpc/server"
import { createRouter } from "@dassie/lib-rpc/server"

import { protectedRoute } from "../../rpc-server/route-types/protected"
import { RegistrationStatusSignal } from "../computed/registration-status"

export const registrationClientRouter = createRouter({
  subscribeRegistrationStatus: protectedRoute.subscription(
    ({ context: { sig } }) => {
      return subscribeToSignal(sig, RegistrationStatusSignal)
    },
  ),
})
