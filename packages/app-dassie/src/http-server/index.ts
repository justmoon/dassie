import { createActor } from "@dassie/lib-reactive"

import type { DassieActorContext } from "../base/types/dassie-base"
import { RedirectToHttpsActor } from "./redirect-to-https"
import { ServeFrontendActor } from "./serve-frontend"

export const HttpServerActor = () =>
  createActor((sig: DassieActorContext) => {
    sig.run(RedirectToHttpsActor)
    sig.run(ServeFrontendActor)
  })
