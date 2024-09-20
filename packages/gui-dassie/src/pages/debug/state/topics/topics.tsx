import { useState } from "react"

import { ScrollArea } from "../../../../components/ui/scroll-area"
import { combine } from "../../../../utils/class-helper"
import type { StateKeys } from "../state"
import { TopicCapture } from "./topic-capture"

interface TopicsProperties {
  stateKeys: StateKeys
}

export function Topics({ stateKeys }: TopicsProperties) {
  const [currentKey, setCurrentKey] = useState<number | undefined>(undefined)

  return (
    <div className="grid grid-cols-[24rem_auto] h-full min-h-0">
      <ScrollArea>
        <ul className="flex flex-col p-2 pr-4 space-y-2 min-h-0">
          {stateKeys
            .filter((tuple) => tuple[2] === "topic")
            .map(([key, name, type]) => (
              <li
                key={key}
                className={combine(
                  "rounded-md p-2 cursor-pointer hover:bg-accent",
                  key === currentKey && "bg-accent",
                )}
                onClick={() => {
                  setCurrentKey(key)
                }}
              >
                {name} ({type})
              </li>
            ))}
        </ul>
      </ScrollArea>
      {currentKey ?
        <TopicCapture key={currentKey} id={currentKey} />
      : null}
    </div>
  )
}
