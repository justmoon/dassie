import { createActor } from "@dassie/lib-reactive"

import { pingBeacons } from "./ping-beacons"

export const startBeaconClient = () =>
  createActor((sig) => {
    sig.run(pingBeacons)
  })
