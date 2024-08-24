import { RuleTester } from "@typescript-eslint/rule-tester"
import { afterAll, describe, it } from "vitest"

RuleTester.afterAll = afterAll
RuleTester.it = it
RuleTester.itOnly = it.only
RuleTester.describe = describe

const fixturePath = new URL("../fixture", import.meta.url).pathname

export const ruleTester = new RuleTester({
  languageOptions: {
    parserOptions: {
      tsconfigRootDir: fixturePath,
      project: "./tsconfig.json",
      projectService: false,
      errorOnTypeScriptSyntacticAndSemanticIssues: true,
      errorOnUnknownASTType: true,
    },
  },
})
