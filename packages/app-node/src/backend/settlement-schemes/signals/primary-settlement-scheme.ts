import { createSignal } from "@dassie/lib-reactive"

import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"

export const PrimarySettlementSchemeSignal = () =>
  createSignal<SettlementSchemeId | undefined>()
