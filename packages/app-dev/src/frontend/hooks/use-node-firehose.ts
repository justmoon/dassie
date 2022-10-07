import { useEffect, useState } from "react"

import { nodeClients } from "../shared-effects/node-rpc-clients"

export interface FirehoseEvent {
  topic: string
  message: string
}

export const useNodeFirehose = (nodeId: string): FirehoseEvent[] => {
  const [events, setEvents] = useState<{ topic: string; message: string }[]>([])

  useEffect(() => {
    const [client, dispose] = nodeClients(nodeId)

    const subscription = client.listenToFirehose.subscribe(undefined, {
      onData(event) {
        setEvents((events) => [...events, event])
      },
      onError(error) {
        console.error(error)
      },
    })

    return () => {
      subscription.unsubscribe()
      dispose()
    }
  }, [nodeId])

  return events
}
