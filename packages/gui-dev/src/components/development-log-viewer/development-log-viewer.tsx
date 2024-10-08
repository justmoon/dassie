import type { ReactNode } from "react"

import {
  type DevelopmentServerLogLine,
  LogsStore,
} from "@dassie/app-dev/src/stores/logs"
import { shortenNodeId } from "@dassie/app-dev/src/utils/shorten-node-id"
import {
  DEFAULT_FORMAT,
  type FormatDefinition,
} from "@dassie/gui-dassie/src/components/log-viewer/default-format"
import LogViewer, {
  LogViewerProvider,
} from "@dassie/gui-dassie/src/components/log-viewer/log-viewer"
import { useRemoteStore } from "@dassie/lib-reactive-rpc/client"

import { rpc } from "../../utils/rpc"
import NodeLink from "../shared/node-link/node-link"
import { formatString } from "./format-string"

interface DevelopmentLogViewerProperties {
  filter?: (line: DevelopmentServerLogLine) => boolean
}

export const DevelopmentLogProvider = ({
  children,
}: {
  children: ReactNode
}) => {
  const openFileMutation = rpc.openFile.useMutation()

  const format: FormatDefinition = {
    ...DEFAULT_FORMAT,
    string: {
      ...DEFAULT_FORMAT.string,
      formatter: formatString,
    },
  }

  return (
    <LogViewerProvider
      openFile={(file) => {
        openFileMutation.mutate(file)
      }}
      renderNodeColumn={(log) => (
        <span className="inline-block font-bold flex-shrink-0 w-9 mr-1">
          {" "}
          {log.node ?
            <NodeLink
              id={log.node.length > 10 ? shortenNodeId(log.node) : log.node}
            />
          : "unknown"}{" "}
        </span>
      )}
      format={format}
    >
      {children}
    </LogViewerProvider>
  )
}

export function DevelopmentLogViewer({
  filter,
}: DevelopmentLogViewerProperties) {
  const { logs } = useRemoteStore(rpc.subscribeToLogs, LogsStore)

  return (
    <DevelopmentLogProvider>
      <LogViewer logs={logs} filter={filter} />
    </DevelopmentLogProvider>
  )
}
