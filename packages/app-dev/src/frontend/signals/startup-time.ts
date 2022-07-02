import { createResource } from "solid-js"

import client from "../rpc-client"

export const [startupTime] = createResource(async () => {
  return await client.query("startupTime")
})
