import { createStore } from "@xen-ilp/lib-reactive"

import { NODES } from "../constants/development-nodes"

export const activeTemplate = () => createStore(NODES)
