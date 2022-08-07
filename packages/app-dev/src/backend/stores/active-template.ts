import { createStore } from "@xen-ilp/lib-reactive"

import { PEERS } from "../constants/development-nodes"

export const activeTemplate = () => createStore(PEERS)
