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

  const typeFormatter = format[typeIdentifier] as TypeInfo

  const value =
    "formatter" in typeFormatter ?
      typeFormatter.formatter(content)
    : String(content)

  const onClick = () => {
    // eslint-disable-next-line no-console
    console.log(content)
  }

  return keyName ?
      <span
        className="cursor-pointer inline-block hover:bg-gray-8 rounded"
        onClick={onClick}
      >
        <span className="font-sans text-gray-3">{keyName}=</span>
        <span
          className={"text-xs inline-block"}
          style={{ color: typeFormatter.color }}
        >
          {value}
        </span>
      </span>
    : <span
        onClick={onClick}
        style={{ color: typeFormatter.color }}
        className={
          "text-xs inline-block cursor-pointer hover:bg-gray-8 rounded"
        }
      >
        {value}
      </span>
}
