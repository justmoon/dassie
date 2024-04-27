import createIgnore from "ignore"
import ts from "typescript"

import { readFileSync } from "node:fs"
import { relative, resolve } from "node:path"

import { printToConsole, reportPackageStatus } from "./utils/report-status"

const PackagePathSymbol = Symbol("PackagePath")
const PackageNameSymbol = Symbol("PackageName")

const currentFilePath = new URL(import.meta.url).pathname
const projectRoot = resolve(currentFilePath, "../../../../")

const ignore = createIgnore().add(
  readFileSync(resolve(projectRoot, ".gitignore")).toString(),
)

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
  const packagesToBeLinted: import("./types/packages-to-be-linted").PackagesToBeLinted =
    []

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
    host.createProgram = (rootNames, options, host, oldProgram) => {
      const builderProgram = originalCreateProgram(
        rootNames,
        options,
        host,
        oldProgram,
      )

      if (typeof options?.["configFilePath"] !== "string") {
        throw new TypeError("Expected options.configFilePath to be a string")
      }

      const packagePath = resolve(options["configFilePath"], "../")
      const packageJsonPath = resolve(packagePath, "package.json")
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
      const packagePath = program[PackagePathSymbol]!
      const packageName = program[PackageNameSymbol]!
      const diagnostics = ts.getPreEmitDiagnostics(program)

      if (diagnostics.length > 0) {
        reportPackageStatus(packageName, "error")
        return
      }

      reportPackageStatus(packageName, "success")

      const sourceFiles = program
        .getSourceFiles()
        .map(({ fileName }) => fileName)
        .filter(
          (fileName) =>
            fileName.startsWith(packagePath) &&
            !ignore.ignores(relative(projectRoot, fileName)),
        )

      if (packageName === "@dassie/eslint-plugin") {
        // We have to skip linting for the ESLint plugin because linting depends
        // on the ESLint configuration, which in turn depends on the ESLint plugin.
        //
        // In other words, linting this package would create a circular dependency.
        return
      }

      packagesToBeLinted.push({
        packagePath,
        packageName,
        sourceFiles,
      })
    }

    return host
  }

  return { packagesToBeLinted, buildResult }
}
