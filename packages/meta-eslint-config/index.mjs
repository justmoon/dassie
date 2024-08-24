import eslint from "@eslint/js"
import eslintConfigPrettier from "eslint-config-prettier"
import eslintPluginN from "eslint-plugin-n"
import eslintPluginReact from "eslint-plugin-react"
import eslintPluginTsdoc from "eslint-plugin-tsdoc"
import eslintPluginUnicorn from "eslint-plugin-unicorn"
import tseslint from "typescript-eslint"

import path from "node:path"

import eslintPluginDassie from "@dassie/eslint-plugin"

export default tseslint.config(
  eslint.configs.recommended,
  ...tseslint.configs.recommendedTypeChecked,
  ...tseslint.configs.strictTypeChecked,
  ...tseslint.configs.stylisticTypeChecked,
  eslintPluginUnicorn.configs["flat/recommended"],
  eslintPluginReact.configs.flat.recommended,
  eslintPluginReact.configs.flat["jsx-runtime"],
  eslintPluginN.configs["flat/recommended-module"],
  eslintPluginDassie.configs.recommended,
  eslintConfigPrettier,
  {
    ignores: [
      "dist/**",
      "coverage/",
      "eslint.config.mjs",
      "next.config.js",
      "rollup.config.js",
    ],
  },
  {
    plugins: { tsdoc: eslintPluginTsdoc },
    rules: {
      "no-console": [
        "warn",
        { allow: ["debug", "info", "warn", "error", "clear"] },
      ],
      "unicorn/no-null": "off",
      "unicorn/no-useless-undefined": "off",
      "tsdoc/syntax": "warn",
      "@typescript-eslint/no-non-null-assertion": "off",
      "@typescript-eslint/no-invalid-void-type": "off",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/no-empty-object-type": "off",
      "@typescript-eslint/restrict-template-expressions": [
        "error",
        { allowNumber: true },
      ],
      "@typescript-eslint/array-type": "off",
      // Buggy rule that causes infinite recursion in a couple of places
      "@typescript-eslint/no-unnecessary-type-parameters": "off",
      "@typescript-eslint/consistent-type-definitions": "off",
      "object-shorthand": ["warn", "properties"],

      "react/prop-types": "off",

      "n/no-missing-import": "off",
      "n/no-unpublished-import": "off",
      "n/no-extraneous-import": "off",
      "n/shebang": "off",
      "n/no-process-exit": "off",

      "@dassie/no-top-level-side-effects": [
        "error",
        {
          allowSimpleAssignments: true,
          allowInExportlessModules: true,
          allowedNodes: [
            // Some libraries require global function calls for configuration
            // - { enableMapSet } from 'immer'
            // - { allowErrorProps, registerClass } from 'superjson'
            "ExpressionStatement:has(CallExpression[callee.name=/^(enableMapSet|allowErrorProps|registerClass)$/])",

            // Allow inline tests via vitest
            'IfStatement:has([name="vitest"])',
          ],
        },
      ],
    },
    languageOptions: {
      ecmaVersion: 2021,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: path.resolve(import.meta.dirname, "../../"),
      },
    },

    settings: {
      react: {
        version: "18.3.1",
      },
    },
  },

  // Overrides
  {
    files: ["**/*.{,c,m}js{,x}"],
    rules: {
      "tsdoc/syntax": "off",
    },
  },
  {
    files: ["packages/app-cli/index.js"],
    env: {
      node: true,
    },
  },
  {
    files: ["next-env.d.ts"],
    rules: {
      "unicorn/prevent-abbreviations": "off",
    },
  },
  {
    files: ["**/*.test.ts{,x}"],
    rules: {
      "unicorn/consistent-function-scoping": "off",
      "@dassie/no-top-level-side-effects": "off",
      "@typescript-eslint/no-empty-function": "off",
      "@typescript-eslint/no-confusing-void-expression": "off",
    },
  },
  {
    files: ["**/examples/**", "**/bin/**"],
    rules: {
      "@dassie/no-top-level-side-effects": "off",
    },
  },
)
