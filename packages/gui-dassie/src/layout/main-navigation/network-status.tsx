import { combine } from "../../utils/class-helper"
import { rpc } from "../../utils/rpc"

interface NetworkStatusAppearanceProperties {
  state: "unknown" | "connected"
  nodeCount: number
}

export const NetworkStatusAppearance = ({
  state,
  nodeCount,
}: NetworkStatusAppearanceProperties) => {
  const stateColor = state === "unknown" ? "bg-gray-400" : "bg-green-400"
  return (
    <span className="inline-flex items-center">
      <div className={combine("h-4 w-4 rounded-full mr-2", stateColor)} />
      {nodeCount}
    </span>
  )
}

export const NetworkStatus = () => {
  const basicState = rpc.general.getBasicState.useQuery(undefined, {
    refetchInterval: 1000,
  })

  if (!basicState.data || basicState.data.state === "uninitialized") {
    return <NetworkStatusAppearance state="unknown" nodeCount={0} />
  }

  return (
    <NetworkStatusAppearance
      state="connected"
      nodeCount={basicState.data.nodeCount}
    />
  )
}
