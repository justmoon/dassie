import { ESLint } from "eslint"

import { unlinkSync } from "node:fs"
import path from "node:path"

import type { PackagesToBeLinted } from "./types/packages-to-be-linted"
import { printToConsole, reportPackageStatus } from "./utils/report-status"

export async function runEslint(packagesToBeLinted: PackagesToBeLinted) {
  const eslint = new ESLint()

  let anyPackageHasErrors = false
  for (const { packagePath, packageName } of packagesToBeLinted) {
    reportPackageStatus(packageName, "start")

    const results = await eslint.lintFiles(packagePath)

    const formatter = await eslint.loadFormatter("stylish")
    const resultText = await formatter.format(results)

    printToConsole(resultText)

    const hasErrors = results.some(
      (result) => result.errorCount > 0 || result.warningCount > 0,
    )

    if (hasErrors) {
      reportPackageStatus(packageName, "error")
      // Delete build cache to trigger rebuild on next run
      unlinkSync(path.resolve(packagePath, "dist/tsconfig.tsbuildinfo"))
      process.exitCode = 1
    } else {
      reportPackageStatus(packageName, "success")
    }

    anyPackageHasErrors ||= hasErrors
  }

  return anyPackageHasErrors
}
