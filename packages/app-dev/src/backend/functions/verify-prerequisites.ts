import chalk from "chalk"
import { $ } from "execa"

import { lookup } from "node:dns/promises"
import { readFile } from "node:fs/promises"

import { isErrorWithCode } from "@dassie/lib-type-utils"

const NODE_VERSION_PATH = new URL(
  "../../../../../.node-version",
  import.meta.url,
).pathname

export const verifyPrerequisites = async () => {
  // Windows is not supported
  {
    if (process.platform === "win32") {
      console.error(
        `prerequisite check failed: unsupported platform

Dassie does not currently support Windows.

${chalk.bold(
  "Consider using Windows Subsystem for Linux (WSL), a container, or a virtual machine.",
)}
`,
      )
      return false
    }
  }

  // Verify Node.js version is correct
  {
    const requiredNodeVersion = await readFile(NODE_VERSION_PATH, "utf8")

    const actualNodeVersion = process.version

    if (actualNodeVersion.trim() !== requiredNodeVersion.trim()) {
      console.error(
        `prerequisite check failed: incorrect node version

Required Node.js version: ${chalk.bold.green(requiredNodeVersion)}
Your Node.js version: ${chalk.bold.red(actualNodeVersion)}

${chalk.bold(
  "We recommend you use a node version manager which supports .node-version",
)}

Here is a list of possible options: https://stackoverflow.com/q/27425852

If you don't want to choose, try fnm: https://github.com/Schniz/fnm
`,
      )
      return false
    }
  }

  // Verify that OpenSSL is installed
  {
    try {
      await $`openssl version`
    } catch (error: unknown) {
      if (isErrorWithCode(error, "ENOENT")) {
        console.error(
          `prerequisite check failed: openssl is not in path

${chalk.bold(
  "Please install OpenSSL from https://github.com/FiloSottile/mkcert",
)}
`,
        )
        return false
      }

      console.error("prerequisite check failed: error while checking mkcert", {
        error,
      })
      return false
    }
  }

  // Verify that mkcert is installed
  {
    try {
      await $`mkcert --version`
    } catch (error: unknown) {
      if (isErrorWithCode(error, "ENOENT")) {
        console.error(
          `prerequisite check failed: mkcert is not in path

${chalk.bold(
  "Please install mkcert from https://github.com/FiloSottile/mkcert",
)}
`,
        )
        return false
      }

      throw error
    }
  }

  // Check that NODE_EXTRA_CA_CERTS is set to the mkcert root certificate
  {
    const { stdout } = await $`mkcert -CAROOT`

    const expectedPath = `${stdout.trim()}/rootCA.pem`

    if (process.env["NODE_EXTRA_CA_CERTS"] !== expectedPath) {
      console.warn(
        `prerequisite check failed: NODE_EXTRA_CA_CERTS is not set to mkcert root certificate

  $(mkcert -CAROOT)/rootCA.pem: ${chalk.bold.green(expectedPath)}
  NODE_EXTRA_CA_CERTS: ${chalk.bold.red(process.env["NODE_EXTRA_CA_CERTS"])}

  ${chalk.bold('Please set NODE_EXTRA_CA_CERTS="$(mkcert -CAROOT)/rootCA.pem"')}
  `,
      )
      return false
    }
  }

  // Check that *.localhost resolves to 127.0.0.1
  {
    try {
      const { address } = await lookup("d999.localhost", { family: 4 })

      if (address !== "127.0.0.1") {
        console.error(
          `prerequisite check failed: *.localhost does not resolve to 127.0.0.1

*.localhost should resolve to: ${chalk.bold.green("127.0.0.1")}
*.localhost actually resolves to: ${chalk.bold.red(address)}
`,
        )
        return false
      }
    } catch (error: unknown) {
      if (isErrorWithCode(error, "ENOTFOUND")) {
        console.error(
          `prerequisite check failed: *.localhost does not resolve to anything

*.localhost should resolve to: ${chalk.bold.green("127.0.0.1")}
*.localhost does not resolve to anything
`,
        )
        return false
      }

      throw error
    }
  }

  return true
}
