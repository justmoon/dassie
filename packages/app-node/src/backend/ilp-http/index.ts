import { createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { RegisterIlpHttpCallbackHandlerActor } from "./register-ilp-http-callback-handler"
import { RegisterIlpHttpHandlerActor } from "./register-ilp-http-handler"
import { SendIlpHttpPacketsActor } from "./send-ilp-http-packets"

export const IlpHttpActor = () =>
  createActor((sig: DassieActorContext) => {
    sig.run(RegisterIlpHttpHandlerActor)
    sig.run(RegisterIlpHttpCallbackHandlerActor)
    sig.run(SendIlpHttpPacketsActor)
  })
