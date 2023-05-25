import { createSignal } from "@dassie/lib-reactive"

import { SubnetId } from "../../peer-protocol/types/subnet-id"

export const activeSubnetsSignal = () => createSignal<SubnetId[]>([])
