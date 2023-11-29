import { createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { RegisterBtpHttpUpgradeActor } from "./register-btp-http-upgrade"

export const BtpServerActor = () =>
  createActor((sig: DassieActorContext) => {
    sig.run(RegisterBtpHttpUpgradeActor)
  })
