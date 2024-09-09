import { enableMapSet, produce } from "immer"
import { useEffect, useRef, useState } from "react"

import type { ViewableLogLine } from "@dassie/app-node/src/frontend/components/log-viewer/log-line"
import LogViewer from "@dassie/app-node/src/frontend/components/log-viewer/log-viewer"
import { Button } from "@dassie/app-node/src/frontend/components/ui/button"
import { type LogEvent, createLogger, getLogContext } from "@dassie/lib-logger"
import type {
  IlpPreparePacket,
  IlpResponsePacket,
} from "@dassie/lib-protocol-ilp"
import {
  type PskEnvironment,
  createClient,
  createServer,
  createTestEnvironment,
} from "@dassie/lib-protocol-stream"
import { createScope } from "@dassie/lib-reactive"
import { unwrapFailure } from "@dassie/lib-type-utils"

import type { StreamConfiguration } from "../StreamConfigurator/StreamConfigurator"
import StreamPacketLog, {
  type IndexedPreparePacketEvent,
} from "./StreamPacketLog/StreamPacketLog"

enableMapSet()

interface StreamSimulationProperties {
  configuration: StreamConfiguration
  onBackClick: () => void
}

export default function StreamSimulator({
  configuration,
  onBackClick,
}: StreamSimulationProperties) {
  const [startTime] = useState(Date.now())
  const logIndexRef = useRef(0)
  const [logs, setLogs] = useState<ViewableLogLine[]>([])

  const packetIndexRef = useRef(0)
  const [packets, setPackets] = useState<IndexedPreparePacketEvent[]>([])
  const [responsePackets, setResponsePackets] = useState(
    new Map<IlpPreparePacket, IlpResponsePacket>(),
  )
  const [pskEnvironment, setPskEnvironment] = useState<PskEnvironment>()

  useEffect(() => {
    const scope = createScope("stream-simulator")
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
      latency: configuration.latency,
      maxPacketsInFlight: configuration.maxPacketsInFlight,

      scope,
      logger: createLogger("network", { context: logContext }),
    })

    environment.topics.prepare.on(scope, ({ sender, packet }) => {
      setPackets((packets) => [
        ...packets,
        { packet, sender, index: packetIndexRef.current++ },
      ])
    })

    environment.topics.response.on(scope, ({ prepare, response }) => {
      setResponsePackets(
        produce((draft) => {
          draft.set(prepare, response)
        }),
      )
    })

    async function simulate() {
      try {
        setLogs([])
        setPackets([])
        setResponsePackets(new Map())

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

        setPskEnvironment(environment.getPskEnvironment(credentials.secret))

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
      scope.dispose().catch((error: unknown) => {
        console.error("error while disposing environment", { error })
      })
    }
  }, [configuration, startTime, logIndexRef, packetIndexRef])
  return (
    <div className="p-6 gap-6 flex flex-col">
      <Button onClick={() => onBackClick()}>Back</Button>
      <StreamPacketLog
        packets={packets}
        responses={responsePackets}
        pskEnvironment={pskEnvironment}
      />
      <LogViewer logs={logs} />
    </div>
  )
}
