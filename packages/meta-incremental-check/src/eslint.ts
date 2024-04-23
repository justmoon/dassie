import { ESLint } from "eslint"

import { unlinkSync } from "node:fs"
import { resolve } from "node:path"

import type { StatusReporter } from "./types/status-reporter"
import type { PackagesToBeLinted } from "./typescript"

export async function runEslint(
  packagesToBeLinted: PackagesToBeLinted,
  reportPackage: StatusReporter,
  print: (message: string) => void,
) {
  const eslint = new ESLint()
  for (const { packagePath, packageName, sourceFiles } of packagesToBeLinted) {
    reportPackage(packageName, "start")

    const results = await eslint.lintFiles(sourceFiles)

    const formatter = await eslint.loadFormatter("stylish")
    const resultText = await formatter.format(results)

    print(resultText)

    const hasErrors = results.some(
      (result) => result.errorCount > 0 || result.warningCount > 0,
    )

    if (hasErrors) {
      reportPackage(packageName, "error")
      // Delete build cache to trigger rebuild on next run
      unlinkSync(resolve(packagePath, "dist/tsconfig.tsbuildinfo"))
      process.exitCode = 1
    } else {
      reportPackage(packageName, "success")
    }
  }
}
