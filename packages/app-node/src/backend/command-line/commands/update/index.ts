import axios from "axios"
import { command } from "cmd-ts"
import { $ } from "execa"

import { cp, mkdir, rm } from "node:fs/promises"

import { Reactor } from "@dassie/lib-reactive"
import { createFlow, header, note } from "@dassie/lib-terminal-graphics"

import { environmentConfigSignal } from "../../../config/environment-config"
import { downloadFile } from "./download-file"

export const updateCommand = (reactor: Reactor) =>
  command({
    name: "update",
    description:
      "This command assists with the initial configuration and setup",
    args: {},
    async handler() {
      const { rootPath, temporaryPath } = reactor
        .use(environmentConfigSignal)
        .read()
      const architecture = process.arch
      const sessionTemporaryPath = `${temporaryPath}/dassie-update-${Date.now()}-${
        process.pid
      }`

      try {
        if (
          !rootPath.endsWith("/current") &&
          !rootPath.endsWith(`/${__DASSIE_VERSION__}`)
        ) {
          throw new Error(
            `DASSIE_ROOT must end with "/current" or "/${__DASSIE_VERSION__}" in order for the auto-updater to work (DASSIE_ROOT=${rootPath})`
          )
        }

        const flow = createFlow()

        flow.show(header({ title: "Dassie Update" }))

        flow.show(note({ title: "Checking latest version..." }))

        const mirrorUrl =
          process.env["DASSIE_MIRROR_URL"] ?? "https://get.dassie.land"

        const metaLatestUrl = `${mirrorUrl}/meta/latest`

        const metaLatestResponse = await axios.get<string>(metaLatestUrl, {
          responseType: "text",
        })
        const latestVersion = metaLatestResponse.data

        if (latestVersion === __DASSIE_VERSION__) {
          flow.show(note({ title: `Already up to date (${latestVersion})` }))
          return
        }

        flow.show(
          note({
            title: `Installing ${latestVersion} (previous: ${__DASSIE_VERSION__})`,
          })
        )

        await mkdir(sessionTemporaryPath, { recursive: true })

        const filename = `dassie-${latestVersion}-linux-${architecture}.tar.gz`
        const localBundlePath = `${sessionTemporaryPath}/${filename}`
        const bundleUrl = `${mirrorUrl}/${latestVersion}/${filename}`

        await downloadFile(bundleUrl, localBundlePath)

        await $`tar -xzf ${localBundlePath} -C ${sessionTemporaryPath}`

        const rootPathParent = rootPath.slice(0, rootPath.lastIndexOf("/"))
        const targetFolder = `${rootPathParent}/${latestVersion}`

        await rm(targetFolder, { recursive: true, force: true })

        await cp(`${sessionTemporaryPath}/dassie`, targetFolder, {
          recursive: true,
        })

        await rm(rootPath, { recursive: true, force: true })
        await $`ln -s ${targetFolder} ${rootPath}`
      } finally {
        await rm(sessionTemporaryPath, { recursive: true, force: true })
      }
    },
  })
