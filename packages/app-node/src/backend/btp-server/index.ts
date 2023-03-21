import { createActor } from "@dassie/lib-reactive"

import { registerBtpHttpUpgrade } from "./register-btp-http-upgrade"

export const startBtpServer = () =>
  createActor((sig) => {
    sig.run(registerBtpHttpUpgrade)
  })
