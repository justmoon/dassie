import type { ContextKeyTuple } from "../../../../backend/trpc-server/routers/debug"
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../../../components/ui/tabs"
import { trpc } from "../../../utils/trpc"
import { Signals } from "./signals/signals"

export type StateKeys = ContextKeyTuple[]

export function State() {
  const stateKeys = trpc.debug.getContextKeys.useQuery().data ?? []

  return (
    <div className="h-full min-h-0">
      <Tabs defaultValue="signals" className="h-full grid grid-rows-[auto_1fr]">
        <TabsList>
          <TabsTrigger value="signals">Signals</TabsTrigger>
          <TabsTrigger value="actors">Actors</TabsTrigger>
          <TabsTrigger value="topics">Topics</TabsTrigger>
          <TabsTrigger value="other">Other</TabsTrigger>
        </TabsList>
        <TabsContent value="signals" className="min-h-0">
          <Signals stateKeys={stateKeys} />
        </TabsContent>
        <TabsContent value="actors">Not yet implemented</TabsContent>
        <TabsContent value="topics">Not yet implemented</TabsContent>
        <TabsContent value="other">Not yet implemented</TabsContent>
      </Tabs>
    </div>
  )
}
