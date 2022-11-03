import { createSignal } from "@dassie/lib-reactive"

export const debugScopesSignal = () =>
  createSignal(process.env["DEBUG"] ?? "das:*,ilp*")
