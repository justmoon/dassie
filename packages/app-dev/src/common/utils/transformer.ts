import { AxiosError } from "axios"
import { allowErrorProps, registerClass } from "superjson"

allowErrorProps("stack", "cause")
registerClass(AggregateError, {
  allowProps: ["errors", "message", "stack", "cause"],
})
registerClass(AxiosError, {
  allowProps: ["code", "errors", "message", "name", "config", "cause"],
})

export { SuperJSON as transformer } from "superjson"
