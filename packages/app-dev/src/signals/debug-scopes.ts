import { createSignal } from "@dassie/lib-reactive"

export const DebugScopesSignal = () =>
  createSignal(process.env["DEBUG"] ?? "das:*,ilp*")
