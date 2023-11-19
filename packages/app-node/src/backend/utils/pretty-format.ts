import { format } from "pretty-format"

import { formatTypedArrayPlugin } from "@dassie/lib-format-utils"

export const prettyFormat = (value: unknown) =>
  format(value, {
    highlight: true,
    plugins: [formatTypedArrayPlugin],
  })
