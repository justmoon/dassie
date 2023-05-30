import { inspect } from "node:util"

import { createCliFormatter } from "../../../formatters/cli-formatter"

export const showComparison = (value: unknown, title: string) => {
  const formatter = createCliFormatter()
  console.log(
    formatter({
      type: "info",
      date: new Date(),
      message: title,
      parameters: [{ value }],
    })
  )

  console.log(
    "Node inspect (for comparison):\n" +
      inspect(value, { depth: Number.POSITIVE_INFINITY, colors: true })
  )
}
