import { createResource } from "solid-js"

import { client } from "../utils/trpc"

export const [startupTime] = createResource(async () => {
  return await client.query("ui.startupTime")
})
