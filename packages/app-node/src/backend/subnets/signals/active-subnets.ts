import { createSignal } from "@dassie/lib-reactive"

export const activeSubnetsSignal = () => createSignal<string[]>([])
