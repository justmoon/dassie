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

    const dassieContext = sig.withBase(createRuntime())
    await dassieContext.run(DaemonActor)

    dassieContext.run(ServeHttpActor)
    dassieContext.run(ServeIpcSocketActor)
    await dassieContext.run(ServeHttpsActor)
  })
