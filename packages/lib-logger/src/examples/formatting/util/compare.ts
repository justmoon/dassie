/* eslint-disable no-console */
import { inspect } from "node:util"

import { createCliFormatter } from "../../../formatters/cli-formatter"

export const showComparison = (value: unknown, title: string) => {
  const formatter = createCliFormatter()
  console.log(
    formatter({
      type: "info",
      namespace: "example",
      date: Date.now(),
      message: title,
      parameters: [{ value }],
    })
  )

  console.log(
    "Node inspect (for comparison):\n" +
      inspect(value, { depth: Number.POSITIVE_INFINITY, colors: true })
  )
}
