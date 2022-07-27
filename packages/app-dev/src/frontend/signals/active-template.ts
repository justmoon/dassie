import { createResource } from "solid-js"

import { client } from "../utils/trpc"

export const [activeTemplate] = createResource(async () => {
  return await client.query("ui.activeTemplate")
})
