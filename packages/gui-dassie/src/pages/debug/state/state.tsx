import type { ContextKeyTuple } from "@dassie/app-dassie/src/rpc-server/routers/debug"

import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../../../components/ui/tabs"
import { rpc } from "../../../utils/rpc"
import { Actors } from "./actors/actors"
import { Signals } from "./signals/signals"
import { Topics } from "./topics/topics"

export type StateKeys = ContextKeyTuple[]

export function State() {
  const stateKeys = rpc.debug.getContextKeys.useQuery().data ?? []

  return (
    <div className="h-full min-h-0">
      <Tabs defaultValue="signals" className="h-full grid grid-rows-[auto_1fr]">
        <TabsList className="justify-start">
          <TabsTrigger value="signals">Signals</TabsTrigger>
          <TabsTrigger value="actors">Actors</TabsTrigger>
          <TabsTrigger value="topics">Topics</TabsTrigger>
        </TabsList>
        <TabsContent value="signals" className="min-h-0">
          <Signals stateKeys={stateKeys} />
        </TabsContent>
        <TabsContent value="actors">
          <Actors stateKeys={stateKeys} />
        </TabsContent>
        <TabsContent value="topics">
          <Topics stateKeys={stateKeys} />
        </TabsContent>
      </Tabs>
    </div>
  )
}
