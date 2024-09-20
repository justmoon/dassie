import { createActor } from "@dassie/lib-reactive"
import { createRuntime } from "@dassie/lib-reactive-io/node"

import { DaemonActor } from "../../../../daemon"
import { LogToConsoleActor } from "./log-to-console"
import { ServeHttpActor } from "./serve-http"
import { ServeHttpsActor } from "./serve-https"
import { ServeIpcSocketActor } from "./serve-ipc"

export const NodejsDaemonActor = () =>
  createActor(async (sig) => {
    sig.run(LogToConsoleActor)

    await sig.withBase(createRuntime()).run(DaemonActor)

    sig.run(ServeHttpActor)
    sig.run(ServeIpcSocketActor)
    sig.run(ServeHttpsActor)
  })
