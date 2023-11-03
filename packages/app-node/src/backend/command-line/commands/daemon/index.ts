import { command } from "cmd-ts"

import { startDaemon } from "../../../daemon"

export const DaemonCommand = () =>
  command({
    name: "daemon",
    description:
      "This command starts the Dassie daemon in the foreground. It is intended to be used by a process manager like systemd.",
    args: {},
    handler() {
      startDaemon()
    },
  })
