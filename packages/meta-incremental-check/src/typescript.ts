import ts from "typescript"

import { readFileSync } from "node:fs"
import path from "node:path"

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

export function runTypeScriptCompiler(projectPath: string) {
  const host = createCustomSolutionBuilderHost()
  const builder = ts.createSolutionBuilder(host, [projectPath], {
    force: false,
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
      const packageJsonPath = path.resolve(packagePath, "package.json")
      const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8")) as {
        name: string
      }
      const packageName = packageJson.name

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
