import prettyFormatUpstream, { PrettyFormatOptions } from "pretty-format"

import { Uint8ArrayFormatPlugin } from "./uint8array-format-plugin"

export const prettyFormat = (
  value: unknown,
  options: PrettyFormatOptions = {}
): string => {
  return prettyFormatUpstream(value, {
    plugins: [Uint8ArrayFormatPlugin],
    highlight: true,
    ...options,
  })
}
