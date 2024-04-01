import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@dassie/app-node/src/frontend/components/ui/tabs"
import { COLORS } from "@dassie/app-node/src/frontend/constants/palette"
import { selectBySeed } from "@dassie/lib-logger"

import { DevelopmentLogViewer } from "../../development-log-viewer/development-log-viewer"

export const HOST_COLOR = selectBySeed(COLORS, "host")
const HostHeader = () => {
  return (
    <header>
      <h1 className="font-bold leading-tight text-3xl px-4">
        <i
          className="rounded-full h-5 mr-4 w-5 inline-block"
          style={{ background: HOST_COLOR }}
        ></i>
        Host
      </h1>
    </header>
  )
}

const HostLogViewer = () => {
  return <DevelopmentLogViewer filter={({ node }) => !node.startsWith("d")} />
}

const HostDetail = () => {
  return (
    <div className="h-screen grid grid-rows-[auto_1fr] gap-4 py-10">
      <HostHeader />
      <Tabs
        defaultValue="logs"
        className="min-h-0 grid grid-rows-[auto_1fr] px-4"
      >
        <TabsList className="justify-start">
          <TabsTrigger value="logs">Logs</TabsTrigger>
        </TabsList>
        <TabsContent value="logs" className="min-h-0">
          <HostLogViewer />
        </TabsContent>
      </Tabs>
    </div>
  )
}

export default HostDetail
