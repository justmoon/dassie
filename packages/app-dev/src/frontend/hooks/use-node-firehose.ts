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

    const cancelSubscription = client.subscription("listenToFirehose", null, {
      onNext(event) {
        if (event.type !== "data") return
        setEvents((events) => [...events, event.data])
      },
      onError(error) {
        console.error(error)
      },
    })

    return () => {
      cancelSubscription()
      dispose()
    }
  }, [nodeId])

  return events
}
