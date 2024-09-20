import { ScrollArea } from "@radix-ui/react-scroll-area"
import { useMemo } from "react"

import type { StateKeys } from "../state"
import { ActorNode } from "./actor-node"

interface ActorsProperties {
  stateKeys: StateKeys
}

export function Actors({ stateKeys }: ActorsProperties) {
  const [actorTree, names] = useMemo(() => {
    const structuredActorMap = new Map<number, number[]>()
    const names = new Map<number, string>()

    names.set(-1, "Reactor")

    for (const [key, name, type, parent] of stateKeys) {
      if (type !== "actor") continue

      names.set(key, name)

      let parentEntry = structuredActorMap.get(parent)
      if (!parentEntry) {
        parentEntry = []
        structuredActorMap.set(parent, parentEntry)
      }

      parentEntry.push(key)
    }

    return [structuredActorMap, names]
  }, [stateKeys])

  return (
    <div className="h-full min-h-0">
      <ScrollArea>
        <ul className="flex flex-col p-2 pr-4 space-y-2 min-h-0">
          <ActorNode id={-1} actorTree={actorTree} names={names} />
        </ul>
      </ScrollArea>
    </div>
  )
}
