import type { ReactNode } from "react"

import { type IndexedLogLine } from "@dassie/app-node/src/common/stores/logs"
import {
  DEFAULT_FORMAT,
  type FormatDefinition,
} from "@dassie/app-node/src/frontend/components/log-viewer/default-format"
import LogViewer, {
  LogViewerProvider,
} from "@dassie/app-node/src/frontend/components/log-viewer/log-viewer"
import { useRemoteStore } from "@dassie/lib-reactive-trpc/client"

import { shortenNodeId } from "../../../backend/utils/shorten-node-id"
import { LogsStore } from "../../../common/stores/logs"
import { trpc } from "../../utils/trpc"
import NodeLink from "../shared/node-link/node-link"
import { formatString } from "./format-string"

interface DevelopmentLogViewerProperties {
  filter?: (line: IndexedLogLine) => boolean
}

export const DevelopmentLogProvider = ({
  children,
}: {
  children: ReactNode
}) => {
  const openFileMutation = trpc.ui.openFile.useMutation()

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
          <NodeLink
            id={log.node.length > 10 ? shortenNodeId(log.node) : log.node}
          />{" "}
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
  const { logs } = useRemoteStore(trpc.ui.subscribeToLogs, LogsStore)

  return (
    <DevelopmentLogProvider>
      <LogViewer logs={logs} filter={filter} />
    </DevelopmentLogProvider>
  )
}
