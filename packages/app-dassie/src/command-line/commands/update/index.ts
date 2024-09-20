import { command, flag, option, string } from "cmd-ts"
import { $ } from "execa"

import {
  access,
  cp,
  constants as fsConstants,
  mkdir,
  rm,
} from "node:fs/promises"
import path from "node:path"

import type { Reactor } from "@dassie/lib-reactive"
import { createFlow, header, note } from "@dassie/lib-terminal-graphics"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { EnvironmentConfig } from "../../../config/environment-config"
import { downloadFile } from "./download-file"

export const UpdateCommand = (reactor: Reactor) =>
  command({
    name: "update",
    description:
      "Update Dassie to the latest version or a specific target version",
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
      const { rootPath, temporaryPath } = reactor.use(EnvironmentConfig)
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

        const superRootPath = path.dirname(rootPath)
        const currentSymlinkPath = path.resolve(superRootPath, "current")
        const oldVersionPath = path.resolve(superRootPath, __DASSIE_VERSION__)

        const flow = createFlow()

        flow.show(header({ title: "Dassie Update" }))

        // Check permissions

        try {
          await access(rootPath, fsConstants.W_OK)
          await access(superRootPath, fsConstants.W_OK)
          await access(currentSymlinkPath, fsConstants.W_OK)
          await access(oldVersionPath, fsConstants.W_OK)
        } catch (error) {
          if (isErrorWithCode(error, "EACCES")) {
            flow.show(
              note({
                style: "error",
                title: `Insufficient permissions`,
                body: `Please run this command as root or with sudo`,
              }),
            )
            return
          }

          throw error
        }

        const mirrorUrl =
          process.env["DASSIE_MIRROR_URL"] ?? "https://get.dassie.land"

        // If no target version provided, determine latest version
        if (!targetVersion) {
          flow.show(note({ title: "Checking latest version..." }))

          const metaLatestUrl = `${mirrorUrl}/meta/latest`

          const metaLatestResponse = await fetch(metaLatestUrl)
          targetVersion = await metaLatestResponse.text()
        }

        const newVersionPath = path.resolve(superRootPath, targetVersion)

        if (!force && targetVersion === __DASSIE_VERSION__) {
          flow.show(note({ title: `Already up to date (${targetVersion})` }))
          return
        }

        flow.show(
          note({
            title: `Downloading ${targetVersion} (previous: ${__DASSIE_VERSION__})`,
          }),
        )

        await mkdir(sessionTemporaryPath, { recursive: true })

        const filename = `dassie-${targetVersion}-linux-${architecture}.tar.gz`
        const localBundlePath = `${sessionTemporaryPath}/${filename}`
        const bundleUrl = `${mirrorUrl}/${targetVersion}/${filename}`

        await downloadFile(bundleUrl, localBundlePath)

        note({
          title: `Extracting`,
        })
        await $`tar -xzf ${localBundlePath} -C ${sessionTemporaryPath}`

        await cp(`${sessionTemporaryPath}/dassie`, newVersionPath, {
          recursive: true,
        })

        if (oldVersionPath !== newVersionPath) {
          flow.show(
            note({
              title: `Removing ${oldVersionPath}`,
            }),
          )
          await rm(oldVersionPath, { recursive: true, force: true })
        }

        // Update symlink
        await rm(currentSymlinkPath)
        await $`ln -s ${newVersionPath} ${currentSymlinkPath}`

        flow.show(
          note({
            title: `Restarting Dassie`,
          }),
        )

        await $`systemctl restart dassie`

        flow.show(
          note({
            style: "success",
            title: `Successfully installed ${targetVersion}`,
          }),
        )
      } finally {
        await rm(sessionTemporaryPath, { recursive: true, force: true })
      }
    },
  })
