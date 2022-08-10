import { createStore } from "@dassie/lib-reactive"

import { PEERS } from "../constants/development-nodes"

export const activeTemplate = () => createStore(PEERS)
