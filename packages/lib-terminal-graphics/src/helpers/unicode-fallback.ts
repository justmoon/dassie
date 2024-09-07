import isUnicodeSupported from "is-unicode-supported"

import type { UnicodeWithFallback } from "../theme"

export const maybeUnicode = (value: UnicodeWithFallback) => {
  if (typeof value === "string") {
    return value
  }

  return isUnicodeSupported() ? value[0] : value[1]
}
