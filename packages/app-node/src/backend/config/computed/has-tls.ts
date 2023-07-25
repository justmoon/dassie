import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigStore, hasTls } from "../database-config"

export const hasTlsComputed = () =>
  createComputed((sig) => {
    const config = sig.get(databaseConfigStore)
    return hasTls(config)
  })
