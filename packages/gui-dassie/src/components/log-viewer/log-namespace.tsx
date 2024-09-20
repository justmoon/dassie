import { selectBySeed } from "@dassie/lib-logger"

import { COLORS } from "../../constants/palette"
import { useLogViewerContext } from "./log-viewer"

export interface LogNamespaceProperties {
  namespace: string
  caller: string | undefined
}

export const LogNamespace = ({ namespace, caller }: LogNamespaceProperties) => {
  const { openFile } = useLogViewerContext()

  return (
    <span
      className={caller ? "hover:underline cursor-pointer" : ""}
      style={{ color: selectBySeed(COLORS, namespace) }}
      onClick={
        caller
          ? () => {
              openFile?.(caller)
            }
          : undefined
      }
    >
      {namespace}
    </span>
  )
}
