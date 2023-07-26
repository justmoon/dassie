import * as Tabs from "@radix-ui/react-tabs"

import { selectBySeed } from "@dassie/lib-logger"

import { COLORS } from "../../constants/palette"
import LogViewer from "../log-viewer/log-viewer"

export const HOST_COLOR = selectBySeed(COLORS, "host")
const HostHeader = () => {
  return (
    <header>
      <div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
        <h1 className="font-bold leading-tight text-3xl text-gray-100">
          <i
            className="rounded-full h-5 mr-4 w-5 inline-block"
            style={{ background: HOST_COLOR }}
          ></i>
          Host
        </h1>
      </div>
    </header>
  )
}

const HostLogViewer = () => {
  return <LogViewer filter={({ node }) => !node.startsWith("n")} />
}

const HostDetail = () => {
  return (
    <div className="h-screen grid grid-rows-[min-content_auto] py-10">
      <HostHeader />
      <Tabs.Root
        defaultValue="logs"
        className="mx-auto w-full min-h-0 max-w-7xl grid grid-rows-[min-content_auto] pt-8 gap-4 sm:px-6 lg:px-8"
      >
        <Tabs.List className="flex flex-wrap -mb-px">
          <Tabs.Trigger
            value="logs"
            className="border-transparent rounded-t-lg border-b-2 p-4 inline-block hover:border-gray-300 hover:text-gray-300"
          >
            Logs
          </Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content
          value="logs"
          className="rounded-lg bg-gray-800 min-h-0 p-4"
        >
          <HostLogViewer />
        </Tabs.Content>
      </Tabs.Root>
    </div>
  )
}

export default HostDetail
