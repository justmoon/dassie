import { expect } from "vitest"

import { formatTypedArrayPlugin } from "@dassie/lib-format-utils"
import { isFailure } from "@dassie/lib-type-utils"

import { ParseFailure, SerializeFailure } from "../../src/utils/failures"

function indentString(message: string, indent: string) {
  return message.replaceAll(/\n(?!$)/g, `\n${indent}`)
}

export const enableSnapshotSerializers = () => {
  expect.addSnapshotSerializer({
    serialize(value, config, indentation, depth, references, printer) {
      if (value instanceof ParseFailure) {
        return `[${value.name}(offset ${value.offset}): ${indentString(
          value.message,
          indentation,
        )}]`
      }
      if (value instanceof SerializeFailure) {
        return `[${value.name}: ${indentString(value.message, indentation)}]`
      }

      return printer(value, config, indentation, depth, references)
    },
    test(value) {
      return isFailure(value)
    },
  })

  expect.addSnapshotSerializer(formatTypedArrayPlugin)
}
