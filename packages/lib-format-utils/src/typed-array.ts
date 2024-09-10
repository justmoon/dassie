import type { Plugin } from "pretty-format"

// eslint-disable-next-line no-restricted-imports
import { isTypedArray } from "node:util/types"

function formatTypedArrayContentsAsHex(
  value: NodeJS.TypedArray,
  indent: string,
): string {
  switch (value.BYTES_PER_ELEMENT) {
    case 8: {
      return (
        Buffer.from(value)
          .toString("hex")
          .replaceAll(/(.{16})/g, "$1 ")
          .replaceAll(/([\da-f]{8})/g, "$1 ")
          // eslint-disable-next-line no-regex-spaces
          .replaceAll(/(.{8} .{8}  .{8} .{8})  /g, `$1\n${indent}`)
          .trim()
      )
    }
    case 4: {
      return Buffer.from(value)
        .toString("hex")
        .replaceAll(/(.{8})/g, "$1 ")
        .replaceAll(/((?:.{8} ){3}.{8}) /g, `$1\n${indent}`)
        .trim()
    }
    case 2: {
      return Buffer.from(value)
        .toString("hex")
        .replaceAll(/(.{4})/g, "$1 ")
        .replaceAll(/((?:.{4} ){7}.{4}) /g, `$1\n${indent}`)
        .trim()
    }
    default: {
      return Buffer.from(value)
        .toString("hex")
        .replaceAll(/(..)/g, "$1 ")
        .replaceAll(/((?:.. ){15}..) /g, `$1\n${indent}`)
        .trim()
    }
  }
}

export const formatTypedArrayPlugin = {
  serialize(value, config, indentation, depth, references, printer) {
    if (isTypedArray(value)) {
      const tag = value[Symbol.toStringTag]

      const hex = formatTypedArrayContentsAsHex(
        value,
        indentation + config.indent,
      )

      return value.length > 16 ?
          `${tag} [\n${indentation}${config.indent}${hex}\n${indentation}]`
        : `${tag} [ ${hex} ]`
    }

    return printer(value, config, indentation, depth, references)
  },
  test(value) {
    return isTypedArray(value)
  },
} satisfies Plugin
