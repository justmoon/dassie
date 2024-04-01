import { isError } from "@dassie/lib-logger"

import type { RecognizedType, TypeInfo } from "./default-format"
import { useLogViewerContext } from "./log-viewer"

interface DataValueProperties {
  keyName?: string
  content: unknown
}

export const DataValue = ({ keyName, content }: DataValueProperties) => {
  const { format } = useLogViewerContext()

  let typeIdentifier: RecognizedType = typeof content

  if (typeIdentifier === "object") {
    if (Array.isArray(content)) {
      typeIdentifier = "array"
    } else if (isError(content)) {
      typeIdentifier = "error"
    } else if (content === null) {
      typeIdentifier = "null"
    }
  }

  const typeFormatter = format[typeIdentifier] as TypeInfo<unknown>

  const value =
    "formatter" in typeFormatter
      ? typeFormatter.formatter(content)
      : String(content)

  // eslint-disable-next-line no-console
  const onClick = () => console.log(content)

  return keyName ? (
    <span
      onClick={onClick}
      className={`bg-dark-100 rounded-1 text-xs px-1 inline-block cursor-pointer`}
    >
      <span className="font-sans text-gray-3">{keyName}=</span>
      <span style={{ color: typeFormatter.color }}>{value}</span>
    </span>
  ) : (
    <span
      onClick={onClick}
      style={{ color: typeFormatter.color }}
      className={`bg-dark-100 rounded-1 text-xs px-1 inline-block cursor-pointer`}
    >
      {value}
    </span>
  )
}
