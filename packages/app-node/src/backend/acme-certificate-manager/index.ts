import { createActor } from "@dassie/lib-reactive"

import type { DassieActorContext } from "../base/types/dassie-base"
import { ServeTokensActor } from "./serve-tokens"
import { UpdateAcmeCertificateActor } from "./update-acme-certificate"

export const AcmeCertificateManagerActor = () =>
  createActor(async (sig: DassieActorContext) => {
    sig.run(ServeTokensActor)
    await sig.run(UpdateAcmeCertificateActor)
  })
