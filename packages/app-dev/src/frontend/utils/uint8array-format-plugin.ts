import { uint8ArrayToHex } from "@dassie/lib-oer"
import type { Plugin } from "pretty-format"

/**
 * Plugin for `pretty-format` to render Uint8Arrays as hex strings.
 */
export const Uint8ArrayFormatPlugin: Plugin = {
  test(value: unknown) {
    return value instanceof Uint8Array
  },

  serialize(value: Uint8Array, { indent }, indentation) {
    const currentIndent = `${indentation}${indent}`
    const hex = uint8ArrayToHex(value).replace(
      /((?:[\dA-Fa-f]{2} ){24})/g,
      `$1\n${currentIndent}`
    )
    return `Uint8Array [\n${currentIndent}${hex}\n${indentation}]`
  },
}
