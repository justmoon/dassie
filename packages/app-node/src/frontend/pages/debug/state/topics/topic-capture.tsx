import { useState } from "react"

import { rpc } from "../../../../utils/rpc"

interface TopicCaptureProperties {
  id: number
}

export function TopicCapture({ id }: TopicCaptureProperties) {
  const [events, setEvents] = useState<string[]>([])
  rpc.debug.subscribeToTopic.useSubscription(id, {
    onData: (data) => {
      setEvents((events) => [...events, data])
    },
  })

  return (
    <div>
      {events.map((event, index) => (
        <div key={index}>
          <pre>{event}</pre>
        </div>
      ))}
      {events.length === 0 && <div>Waiting for events...</div>}
    </div>
  )
}
