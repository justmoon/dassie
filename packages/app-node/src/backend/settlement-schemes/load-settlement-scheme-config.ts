import { createActor } from "@dassie/lib-reactive"

import { environmentConfigSignal } from "../config/environment-config"
import { activeSettlementSchemesSignal } from "./signals/active-settlement-schemes"
import { primarySettlementSchemeSignal } from "./signals/primary-settlement-scheme"
import { settlementSchemeMapSignal } from "./signals/settlement-scheme-map"

export const loadSettlementSchemeConfig = () =>
  createActor((sig) => {
    const { initialSettlementSchemes } = sig.use(environmentConfigSignal).read()
    const settlementSchemeMap = sig.get(settlementSchemeMapSignal)

    for (const scheme of initialSettlementSchemes) {
      settlementSchemeMap.set(scheme.id, {
        settlementSchemeId: scheme.id,
        config: scheme.config,
        initialPeers: scheme.initialPeers ?? [],
      })
    }

    sig
      .use(activeSettlementSchemesSignal)
      .write(
        new Set(
          initialSettlementSchemes.map(
            (settlementScheme) => settlementScheme.id
          )
        )
      )

    // TODO: Primary settlement scheme should be intelligently managed
    sig
      .use(primarySettlementSchemeSignal)
      .write(initialSettlementSchemes[0]?.id)
  })
