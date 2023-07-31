import { selectBySeed } from "@dassie/lib-logger"

import { COLORS } from "../../constants/palette"
import { trpc } from "../../utils/trpc"

export interface LogNamespaceProperties {
  namespace: string
  caller: string | undefined
}

export const LogNamespace = ({ namespace, caller }: LogNamespaceProperties) => {
  const openFileMutation = trpc.ui.openFile.useMutation()
  return (
    <span
      className={caller ? "hover:underline cursor-pointer" : ""}
      style={{ color: selectBySeed(COLORS, namespace) }}
      onClick={
        caller
          ? () => {
              openFileMutation.mutate(caller)
            }
          : undefined
      }
    >
      {namespace}
    </span>
  )
}
