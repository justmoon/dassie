#!/usr/bin/env node
import { ViteNodeRunner } from "vite-node/client"

import { assertDefined } from "@xen-ilp/lib-type-utils"

import { trpcClientFactory } from "./services/trpc-client"

const trpcClient = trpcClientFactory()

assertDefined(process.env["XEN_DEV_ROOT"])
assertDefined(process.env["XEN_DEV_BASE"])

const runner = new ViteNodeRunner({
  root: process.env["XEN_DEV_ROOT"],
  base: process.env["XEN_DEV_BASE"],
  async fetchModule(id) {
    return await trpcClient.query("runner.fetchModule", [id])
  },
  async resolveId(id, importer) {
    return await trpcClient.query("runner.resolveId", [id, importer])
  },
})

// We would like to inject the existing trpcClient instance into the rest of the application. If you can figure out a cleaner way to do this, feel free to make a PR.
// eslint-disable-next-line @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any
;(global as any).trpcClient = trpcClient

// send message to indicate that we're up and running
process.send?.({})

// load vite environment
await runner.executeId("/@vite/env")

// execute the file
assertDefined(process.env["XEN_DEV_ENTRY"])
await runner.executeId(process.env["XEN_DEV_ENTRY"])
