import { createStore } from "@dassie/lib-reactive"

import { BEACON_COUNT } from "../constants/development-beacons"
import { generateBeaconConfig } from "../utils/generate-beacon-config"

export const beaconUrlsStore = () =>
  createStore(
    Array.from({ length: BEACON_COUNT })
      .fill(null)
      .map((_, index) => generateBeaconConfig(index).url)
  )
