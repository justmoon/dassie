import { trpc } from "../../../../utils/trpc"

interface SignalStateProperties {
  id: number
}

export function SignalState({ id }: SignalStateProperties) {
  const signalState = trpc.debug.getSignalState.useQuery(id)
  console.info(signalState.data?.value)

  return (
    <div>
      See state in console. Note that some types cannot be serialized and
      therefore will show as undefined.
    </div>
  )
}
