import { createActor } from "@dassie/lib-reactive"

import { NotifySystemdActor } from "./notify"

export const SystemdActor = () =>
  createActor((sig) => {
    sig.run(NotifySystemdActor)
  })
