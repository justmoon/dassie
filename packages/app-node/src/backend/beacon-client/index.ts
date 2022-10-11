import type { EffectContext } from "@dassie/lib-reactive"

import { pingBeacons } from "./ping-beacons"

export const startBeaconClient = (sig: EffectContext) => {
  sig.run(pingBeacons)
}
