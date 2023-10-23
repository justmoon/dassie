import axios from "axios"
import { command, flag, option, string } from "cmd-ts"
import { $ } from "execa"

import { cp, mkdir, rm } from "node:fs/promises"
import { dirname, resolve } from "node:path"

import { Reactor } from "@dassie/lib-reactive"
import { createFlow, header, note } from "@dassie/lib-terminal-graphics"

import { EnvironmentConfigSignal } from "../../../config/environment-config"
import { downloadFile } from "./download-file"

export const updateCommand = (reactor: Reactor) =>
  command({
    name: "update",
    description:
      "This command assists with the initial configuration and setup",
    args: {
      force: flag({
        long: "force",
        short: "f",
        description:
          "Download and install target version even if same as currently installed",
      }),
      targetVersion: option({
        long: "target-version",
        short: "t",
        type: string,
        description: "Target version to install, e.g. '0.0.1'",
        defaultValue: () => "",
      }),
    },
    async handler({ force, targetVersion }) {
      const { rootPath, temporaryPath } = reactor
        .use(EnvironmentConfigSignal)
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
            `DASSIE_ROOT must end with "/current" or "/${__DASSIE_VERSION__}" in order for the auto-updater to work (DASSIE_ROOT=${rootPath})`,
          )
        }

        const superRootPath = dirname(rootPath)
        const currentSymlinkPath = resolve(superRootPath, "current")
        const oldVersionPath = resolve(superRootPath, __DASSIE_VERSION__)

        const flow = createFlow()

        flow.show(header({ title: "Dassie Update" }))

        flow.show(note({ title: "Checking latest version..." }))

        const mirrorUrl =
          process.env["DASSIE_MIRROR_URL"] ?? "https://get.dassie.land"

        // If no target version provided, determine latest version
        if (!targetVersion) {
          const metaLatestUrl = `${mirrorUrl}/meta/latest`

          const metaLatestResponse = await axios.get<string>(metaLatestUrl, {
            responseType: "text",
          })
          targetVersion = metaLatestResponse.data
        }

        const newVersionPath = resolve(superRootPath, targetVersion)

        if (!force && targetVersion === __DASSIE_VERSION__) {
          flow.show(note({ title: `Already up to date (${targetVersion})` }))
          return
        }

        flow.show(
          note({
            title: `Installing ${targetVersion} (previous: ${__DASSIE_VERSION__})`,
          }),
        )

        await mkdir(sessionTemporaryPath, { recursive: true })

        const filename = `dassie-${targetVersion}-linux-${architecture}.tar.gz`
        const localBundlePath = `${sessionTemporaryPath}/${filename}`
        const bundleUrl = `${mirrorUrl}/${targetVersion}/${filename}`

        await downloadFile(bundleUrl, localBundlePath)

        await $`tar -xzf ${localBundlePath} -C ${sessionTemporaryPath}`

        await cp(`${sessionTemporaryPath}/dassie`, newVersionPath, {
          recursive: true,
        })

        if (oldVersionPath !== newVersionPath) {
          await rm(oldVersionPath, { recursive: true, force: true })
        }

        // Update symlink
        await rm(currentSymlinkPath)
        await $`ln -s ${newVersionPath} ${currentSymlinkPath}`
      } finally {
        await rm(sessionTemporaryPath, { recursive: true, force: true })
      }
    },
  })
