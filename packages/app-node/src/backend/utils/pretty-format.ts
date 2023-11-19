import { format } from "pretty-format"

export const prettyFormat = (value: unknown) =>
  format(value, {
    highlight: true,
  })
