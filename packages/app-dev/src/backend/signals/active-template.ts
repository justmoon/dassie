import { createSignal } from "@dassie/lib-reactive"

import { PEERS } from "../constants/development-nodes"

export const activeTemplateSignal = () => createSignal(PEERS)
