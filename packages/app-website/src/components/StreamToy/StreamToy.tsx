import { useState } from "react"

import StreamConfigurator, {
  DEFAULT_STREAM_CONFIGURATION,
  type StreamConfiguration,
} from "./StreamConfigurator/StreamConfigurator"
import StreamSimulator from "./StreamSimulator/StreamSimulator"

export default function StreamToy() {
  const [simulationConfiguration, setSimulationConfiguration] =
    useState<StreamConfiguration>(DEFAULT_STREAM_CONFIGURATION)

  const [isRunning, setIsRunning] = useState(false)

  function onStartClick() {
    setIsRunning(true)
  }

  function onBackClick() {
    setIsRunning(false)
  }

  return isRunning ?
      <StreamSimulator
        configuration={simulationConfiguration}
        onBackClick={onBackClick}
      />
    : <StreamConfigurator
        configuration={simulationConfiguration}
        onConfigurationChange={setSimulationConfiguration}
        onStartClick={onStartClick}
      />
}
