import { PauseIcon, PlayIcon } from "lucide-react"

import LogViewer from "@dassie/app-node/src/frontend/components/log-viewer/log-viewer"
import { Button } from "@dassie/app-node/src/frontend/components/ui/button"

import type { StreamConfiguration } from "../stream-configurator/stream-configurator"
import StreamPacketLog from "./stream-packet-log/stream-packet-log"
import { useSimulation } from "./use-simulation"

interface StreamSimulationProperties {
  configuration: StreamConfiguration
  onBackClick: () => void
}

export default function StreamSimulator({
  configuration,
  onBackClick,
}: StreamSimulationProperties) {
  const {
    logs,
    packets,
    responsePackets,
    pskEnvironment,
    clockState,
    pause,
    resume,
  } = useSimulation({
    configuration,
  })

  return (
    <div className="p-6 gap-6 flex flex-col">
      <div className="flex flex-row gap-3">
        <Button
          onClick={() => {
            onBackClick()
          }}
        >
          Back
        </Button>
        {clockState.isPaused ?
          <Button onClick={resume}>
            <PlayIcon className="size-4" />
          </Button>
        : <Button onClick={pause}>
            <PauseIcon className="size-4" />
          </Button>
        }
      </div>
      <StreamPacketLog
        packets={packets}
        responses={responsePackets}
        pskEnvironment={pskEnvironment}
      />
      <LogViewer logs={logs} />
    </div>
  )
}
