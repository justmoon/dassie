import { allowErrorProps, registerClass } from "superjson"

allowErrorProps("stack", "cause")
registerClass(TypeError, {
  allowProps: ["message", "stack", "cause"],
})
registerClass(AggregateError, {
  allowProps: ["errors", "message", "stack", "cause"],
})

export { SuperJSON as transformer } from "superjson"
