import { ESLintUtils } from "@typescript-eslint/utils"

export const createRule = ESLintUtils.RuleCreator(
  (name) => `https://dassie.land/rule/${name}`,
)
