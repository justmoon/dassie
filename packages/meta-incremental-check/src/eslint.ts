import { ESLint } from "eslint"

import { mkdir, unlink, writeFile } from "node:fs/promises"
import path from "node:path"

import type { RunChecksOptions } from "."
import { WORKSPACE_ROOT_PATH } from "./constants/workspace-path"
import { getFileDate } from "./utils/get-file-date"
import { getPackages } from "./utils/get-packages"
import { printToConsole, reportPackageStatus } from "./utils/report-status"

const IGNORE_PACKAGES = new Set([
  "meta-api-extractor",
  "meta-eslint-config",
  "meta-tsconfig",
])

export async function runEslint({ all = false }: RunChecksOptions) {
  const eslint = new ESLint({ cwd: WORKSPACE_ROOT_PATH })

  const packageLockDate =
    (await getFileDate(path.resolve(WORKSPACE_ROOT_PATH, "pnpm-lock.yaml"))) ??
    Number.POSITIVE_INFINITY

  async function lintPackage(
    packageName: string,
    packagePath: string,
  ): Promise<boolean> {
    try {
      const typescriptBuildDate =
        (await getFileDate(
          path.resolve(packagePath, "dist/tsconfig.tsbuildinfo"),
        )) ?? Number.POSITIVE_INFINITY

      const referenceDate = Math.max(packageLockDate, typescriptBuildDate)

      const lintTag =
        (await getFileDate(
          path.resolve(packagePath, "dist/incremental-lint-tag"),
        )) ?? 0

      if (!all && lintTag > referenceDate) return false

      reportPackageStatus(packageName, "start")

      const results = await eslint.lintFiles(packagePath)

      const formatter = await eslint.loadFormatter("stylish")
      const resultText = await formatter.format(results, {
        cwd: packagePath,
        rulesMeta: {},
      })

      printToConsole(resultText)

      return results.some(
        (result) => result.errorCount > 0 || result.warningCount > 0,
      )
    } catch (error: unknown) {
      printToConsole(`error while linting package: ${String(error)}`)
      return true
    }
  }

  let anyPackageHasErrors = false
  for await (const packageName of getPackages()) {
    if (IGNORE_PACKAGES.has(packageName)) continue

    const packagePath = path.resolve(
      WORKSPACE_ROOT_PATH,
      "packages",
      packageName,
    )
    const hasErrors = await lintPackage(packageName, packagePath)

    if (hasErrors) {
      reportPackageStatus(packageName, "error")
      process.exitCode = 1

      // Delete incremental lint tag
      await unlink(
        path.resolve(packagePath, "dist/incremental-lint-tag"),
      ).catch(() => {
        /* ignore */
      })
    } else {
      reportPackageStatus(packageName, "success")

      // Update incremental lint tag
      await mkdir(path.resolve(packagePath, "dist"), { recursive: true })
      await writeFile(
        path.resolve(packagePath, "dist/incremental-lint-tag"),
        "",
      )
    }

    anyPackageHasErrors ||= hasErrors
  }

  return anyPackageHasErrors
}
