import { useEffect, useRef, useState } from "react"

import type { ViewableLogLine } from "@dassie/app-node/src/frontend/components/log-viewer/log-line"
import LogViewer from "@dassie/app-node/src/frontend/components/log-viewer/log-viewer"
import { Button } from "@dassie/app-node/src/frontend/components/ui/button"
import { type LogEvent, createLogger, getLogContext } from "@dassie/lib-logger"
import {
  createClient,
  createServer,
  createTestEnvironment,
} from "@dassie/lib-protocol-stream"
import { unwrapFailure } from "@dassie/lib-type-utils"

import type { StreamConfiguration } from "../StreamConfigurator/StreamConfigurator"

interface StreamSimulationProperties {
  configuration: StreamConfiguration
  onBackClick: () => void
}

export default function StreamSimulation({
  configuration,
  onBackClick,
}: StreamSimulationProperties) {
  const [startTime] = useState(Date.now())
  const logIndexRef = useRef(0)
  const [logs, setLogs] = useState<ViewableLogLine[]>([])

  useEffect(() => {
    const logContext = {
      ...getLogContext(),
      enableChecker: () => true,
      output: (logEvent: LogEvent) => {
        if (logEvent.type === "clear") {
          setLogs([])
          return
        }

        setLogs((logs) => [
          ...logs,
          {
            ...logEvent,
            index: logIndexRef.current++,
            relativeTime: Date.now() - startTime,
          },
        ])
      },
    }

    const environment = createTestEnvironment({
      maxPacketAmount: configuration.maxPacketAmount,
      logger: createLogger("network", { context: logContext }),
    })

    async function simulate() {
      try {
        setLogs([])

        const server = unwrapFailure(
          await createServer({
            context: {
              ...environment.createContext({ name: "server" }),
              logger: createLogger("receiver", { context: logContext }),
            },
          }),
        )

        server.on("connection", (connection) => {
          connection.on("stream", (stream) => {
            stream.addReceiveAmount(configuration.amount)
          })
        })

        const credentials = server.generateCredentials()

        const client = unwrapFailure(
          await createClient({
            context: {
              ...environment.createContext({ name: "client" }),
              logger: createLogger("sender", { context: logContext }),
            },
            credentials,
          }),
        )

        const stream = client.createStream()

        await stream.send({ amount: configuration.amount })

        console.log("done")
      } catch (error: unknown) {
        console.error("error while sending", { error })
      }
    }

    simulate().catch((error: unknown) => {
      console.error("error while simulating", { error })
    })

    return () => {
      environment.dispose().catch((error: unknown) => {
        console.error("error while disposing environment", { error })
      })
    }
  }, [configuration, startTime])
  return (
    <div className="p-6 gap-6 flex flex-col">
      <Button onClick={() => onBackClick()}>Back</Button>
      <LogViewer logs={logs} />
    </div>
  )
}
