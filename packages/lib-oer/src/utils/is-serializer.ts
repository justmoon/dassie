import type { Serializer } from "../base-type"

export const isSerializer = (input: unknown): input is Serializer => {
  return (
    typeof input === "function" &&
    typeof (input as Serializer).size === "number"
  )
}
