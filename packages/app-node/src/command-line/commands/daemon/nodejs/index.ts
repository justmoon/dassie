import { createActor } from "@dassie/lib-reactive"
import { createNodeRuntime } from "@dassie/lib-reactive-io/node"

import { DaemonActor } from "../../../../backend/daemon"
import { LogToConsoleActor } from "./log-to-console"
import { ServeHttpActor } from "./serve-http"
import { ServeHttpsActor } from "./serve-https"
import { ServeIpcSocketActor } from "./serve-ipc"

export const NodejsDaemonActor = () =>
  createActor(async (sig) => {
    sig.run(LogToConsoleActor)

    await sig.withBase(createNodeRuntime()).run(DaemonActor)

    sig.run(ServeHttpActor)
    sig.run(ServeIpcSocketActor)
    sig.run(ServeHttpsActor)
  })
