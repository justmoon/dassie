import { createActor } from "@dassie/lib-reactive"

import { RegisterBtpHttpUpgradeActor } from "./register-btp-http-upgrade"

export const BtpServerActor = () =>
  createActor((sig) => {
    sig.run(RegisterBtpHttpUpgradeActor)
  })
