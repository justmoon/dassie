import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigStore, hasNodeIdentity } from "../database-config"

export const hasNodeIdentityComputed = () =>
  createComputed((sig) => {
    const config = sig.get(databaseConfigStore)
    return hasNodeIdentity(config)
  })
