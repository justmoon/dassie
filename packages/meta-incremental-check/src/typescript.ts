import ts from "typescript"

import path from "node:path"

import type { RunChecksOptions } from "."
import { WORKSPACE_ROOT_PATH } from "./constants/workspace-path"
import { getPackageName } from "./utils/get-package-name"
import { printToConsole, reportPackageStatus } from "./utils/report-status"

const PackagePathSymbol = Symbol("PackagePath")
const PackageNameSymbol = Symbol("PackageName")

const typescriptSystem = {
  ...ts.sys,
  write: printToConsole,
}

declare module "typescript" {
  interface Program {
    [PackagePathSymbol]?: string
    [PackageNameSymbol]?: string
  }
}

export function runTypeScriptCompiler({ all }: RunChecksOptions) {
  const host = createCustomSolutionBuilderHost()
  const builder = ts.createSolutionBuilder(host, [WORKSPACE_ROOT_PATH], {
    force: all,
    verbose: false,
  })

  const buildResult = builder.build()

  function createCustomSolutionBuilderHost() {
    const host = ts.createSolutionBuilderHost(
      typescriptSystem,
      undefined,
      // @ts-expect-error createDiagnosticReporter is not exported for some reason
      // eslint-disable-next-line @typescript-eslint/no-unsafe-argument, @typescript-eslint/no-unsafe-call
      ts.createDiagnosticReporter(typescriptSystem, true),
    )

    const originalCreateProgram = host.createProgram
    host.createProgram = (rootNames, options, ...parameters) => {
      const builderProgram = originalCreateProgram(
        rootNames,
        options,
        ...parameters,
      )

      if (typeof options?.["configFilePath"] !== "string") {
        throw new TypeError("Expected options.configFilePath to be a string")
      }

      const packagePath = path.resolve(options["configFilePath"], "../")
      const packageName = getPackageName(packagePath)

      const program = builderProgram.getProgram()
      program[PackagePathSymbol] = packagePath
      program[PackageNameSymbol] = packageName

      reportPackageStatus(packageName, "start")

      return builderProgram
    }

    const originalAfterProgramEmitAndDiagnostics =
      host.afterProgramEmitAndDiagnostics?.bind(host)
    host.afterProgramEmitAndDiagnostics = (builderProgram) => {
      originalAfterProgramEmitAndDiagnostics?.(builderProgram)

      const program = builderProgram.getProgram()
      const packageName = program[PackageNameSymbol]!
      const diagnostics = ts.getPreEmitDiagnostics(program)

      if (diagnostics.length > 0) {
        reportPackageStatus(packageName, "error")
        return
      }

      reportPackageStatus(packageName, "success")
    }

    return host
  }

  return buildResult
}
