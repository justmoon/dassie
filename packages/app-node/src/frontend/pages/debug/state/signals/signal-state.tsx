import { trpc } from "../../../../utils/trpc"

interface SignalStateProperties {
  id: number
}

export function SignalState({ id }: SignalStateProperties) {
  const signalState = trpc.debug.getSignalState.useQuery(id)

  return (
    <div>
      <pre>{signalState.data?.value ?? "Loading..."}</pre>
    </div>
  )
}
