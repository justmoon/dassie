import { createResource } from "solid-js"

import { client } from "../utils/trpc"

export const [config] = createResource(async () => {
  return await client.query("ui.config")
})
