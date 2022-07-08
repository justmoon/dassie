import { createStore } from "@xen-ilp/lib-reactive"

export const activeTemplate = createStore<string>("activeTemplate", "simple")
