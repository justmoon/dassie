import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigStore } from "../../config/database-config"
import { setupAuthorizationTokenSignal } from "../signals/setup-authorization-token"

export const setupUrlComputed = () =>
  createComputed((sig) => {
    const token = sig.get(setupAuthorizationTokenSignal)
    const hostname = sig.get(databaseConfigStore).hostname

    return `https://${hostname}/setup/${token}`
  })
