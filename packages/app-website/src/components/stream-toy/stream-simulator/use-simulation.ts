import { enableMapSet, produce } from "immer"
import { useEffect, useState } from "react"

import type { ViewableLogLine } from "@dassie/gui-dassie/src/components/log-viewer/log-line"
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
import {
  createScope,
  createTimeDilationClock,
  createTopic,
} from "@dassie/lib-reactive"
import { createClock } from "@dassie/lib-reactive-io"
import { UnreachableCaseError, unwrapFailure } from "@dassie/lib-type-utils"

import type { StreamConfiguration } from "../stream-configurator/stream-configurator"
import { type IndexedPreparePacketEvent } from "./stream-packet-log/stream-packet-log"

enableMapSet()

interface SimulationProperties {
  configuration: StreamConfiguration
}

interface PauseClockEvent {
  type: "pause"
}

interface ResumeClockEvent {
  type: "resume"
}

type ClockEvent = PauseClockEvent | ResumeClockEvent

export function useSimulation({ configuration }: SimulationProperties) {
  const [clockState, setClockState] = useState({ isPaused: false })
  const [clockEvents] = useState(createTopic<ClockEvent>())
  const [logs, setLogs] = useState<ViewableLogLine[]>([])
  const [packets, setPackets] = useState<IndexedPreparePacketEvent[]>([])
  const [responsePackets, setResponsePackets] = useState(
    new Map<IlpPreparePacket, IlpResponsePacket>(),
  )
  const [pskEnvironment, setPskEnvironment] = useState<PskEnvironment>()

  useEffect(() => {
    const scope = createScope("stream-simulator")
    const clock = createTimeDilationClock(
      createClock(),
      configuration.timeDilationFactor,
    )

    const startTime = clock.now()

    clockEvents.on(scope, (event) => {
      switch (event.type) {
        case "pause": {
          clock.pause()
          break
        }
        case "resume": {
          clock.resume()
          break
        }
        default: {
          throw new UnreachableCaseError(event)
        }
      }
    })

    let logIndex = 0
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
            index: logIndex++,
            relativeTime: clock.now() - startTime,
          },
        ])
      },
    }

    const environment = createTestEnvironment({
      maxPacketAmount: configuration.maxPacketAmount,
      latency: configuration.latency,
      jitter: configuration.jitter,
      maxPacketsInFlight: configuration.maxPacketsInFlight,

      scope,
      clock,
      logger: createLogger("network", { context: logContext }),
    })

    let packetIndex = 0
    environment.topics.prepare.on(scope, ({ sender, packet }) => {
      setPackets((packets) => [
        ...packets,
        { packet, sender, index: packetIndex++ },
      ])
    })

    environment.topics.response.on(scope, ({ prepare, response }) => {
      setResponsePackets(
        produce((draft) => {
          draft.set(prepare, response)
        }),
      )
    })

    setLogs([])
    setPackets([])
    setResponsePackets(new Map())

    async function simulate() {
      try {
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

        client.dangerouslyIgnoreExchangeRate()

        const stream = client.createStream()

        unwrapFailure(await stream.send({ amount: configuration.amount }))

        await environment.dispose()
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
  }, [configuration, clockEvents])

  return {
    logs,
    packets,
    responsePackets,
    pskEnvironment,

    clockState,
    pause: () => {
      setClockState({ isPaused: true })
      clockEvents.emit({ type: "pause" })
    },
    resume: () => {
      setClockState({ isPaused: false })
      clockEvents.emit({ type: "resume" })
    },
  }
}
