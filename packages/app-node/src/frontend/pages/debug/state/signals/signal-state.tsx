import { rpc } from "../../../../utils/rpc"

interface SignalStateProperties {
  id: number
}

export function SignalState({ id }: SignalStateProperties) {
  const signalState = rpc.debug.getSignalState.useQuery(id)

  return (
    <div>
      <pre>{signalState.data?.value ?? "Loading..."}</pre>
    </div>
  )
}
