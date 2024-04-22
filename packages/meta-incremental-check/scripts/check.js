#!/usr/bin/env node
import { ESLint } from "eslint"
import { readFileSync, unlinkSync } from "fs"
import createIgnore from "ignore"
import { relative, resolve } from "path"
import ts from "typescript"

const PackagePathSymbol = Symbol("PackagePath")
const PackageNameSymbol = Symbol("PackageName")

const currentFilePath = new URL(import.meta.url).pathname
const projectRoot = resolve(currentFilePath, "../../../../")
const packagesToBeLinted = []

const ignore = createIgnore().add(
  readFileSync(resolve(projectRoot, ".gitignore")).toString(),
)

function createCustomSolutionBuilderHost() {
  const host = ts.createSolutionBuilderHost(
    ts.sys,
    null,
    ts.createDiagnosticReporter(ts.sys, true),
  )

  const originalCreateProgram = host.createProgram
  host.createProgram = (rootNames, options, host, oldProgram) => {
    const program = originalCreateProgram(rootNames, options, host, oldProgram)

    const packagePath = resolve(options.configFilePath, "../")
    const packageJsonPath = resolve(packagePath, "package.json")
    const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8"))
    const packageName = packageJson.name

    program[PackagePathSymbol] = packagePath
    program[PackageNameSymbol] = packageName

    console.log("Typechecking", packageName)

    return program
  }

  const originalAfterProgramEmitAndDiagnostics =
    host.afterProgramEmitAndDiagnostics
  host.afterProgramEmitAndDiagnostics = (builderProgram) => {
    originalAfterProgramEmitAndDiagnostics?.(builderProgram)

    console.log()

    const program = builderProgram.getProgram()
    const diagnostics = ts.getPreEmitDiagnostics(program)

    if (diagnostics.length > 0) {
      return
    }

    const sourceFiles = program
      .getSourceFiles()
      .map(({ fileName }) => fileName)
      .filter((fileName) => !ignore.ignores(relative(projectRoot, fileName)))

    const packagePath = builderProgram[PackagePathSymbol]
    const packageName = builderProgram[PackageNameSymbol]

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

function buildProject(projectPath) {
  const host = createCustomSolutionBuilderHost()
  const builder = ts.createSolutionBuilder(host, [projectPath], {
    force: false,
    verbose: false,
    reportDiagnostic: ts.createDiagnosticReporter(ts.sys, true),
  })

  const buildResult = builder.build()
  process.exitCode = buildResult
}

buildProject("./")

if (process.exitCode === 0 && packagesToBeLinted.length > 0) {
  const eslint = new ESLint()
  for (const { packagePath, packageName, sourceFiles } of packagesToBeLinted) {
    console.log("Linting", packageName)

    const results = await eslint.lintFiles(sourceFiles)

    const formatter = await eslint.loadFormatter("stylish")
    const resultText = formatter.format(results)

    console.log(resultText)

    const hasErrors = results.some(
      (result) => result.errorCount > 0 || result.warningCount > 0,
    )

    if (hasErrors) {
      // Delete build cache to trigger rebuild on next run
      unlinkSync(resolve(packagePath, "dist/tsconfig.tsbuildinfo"))
      process.exitCode = 1
    }
  }
}
