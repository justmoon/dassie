import eslintPluginComments from "@eslint-community/eslint-plugin-eslint-comments/configs"
import eslint from "@eslint/js"
import eslintConfigPrettier from "eslint-config-prettier"
import eslintPluginN from "eslint-plugin-n"
import eslintPluginReact from "eslint-plugin-react"
import eslintPluginReactHooks from "eslint-plugin-react-hooks"
import eslintPluginTsdoc from "eslint-plugin-tsdoc"
import eslintPluginUnicorn from "eslint-plugin-unicorn"
import globals from "globals"
import tseslint from "typescript-eslint"

import path from "node:path"

import eslintPluginDassie from "@dassie/meta-eslint-plugin"

const FRONTEND_FILES = [
  "packages/app-website/src/**/*.ts{,x}",
  "packages/gui-dev/src/**/*.ts{,x}",
  "packages/gui-dassie/src/**/*.ts{,x}",
  "packages/lib-reactive-io/src/browser/**/*.ts",
]
const BACKEND_FILES = [
  "packages/app-cli/index.js",
  "packages/app-build/bin/**/*.ts",
  "packages/app-build/src/**/*.ts",
  "packages/app-dassie/src/**/*.ts",
  "packages/app-dev/bin/**/*.{js,ts}",
  "packages/app-dev/src/**/*.{js,ts}",
  "packages/lib-http-server/src/environments/nodejs/**/*.ts",
  "packages/lib-reactive-io/src/node/**/*.ts",
  "packages/lib-terminal-graphics/src/**/*.ts",
  "packages/meta-incremental-check/src/**/*.{js,ts}",
]

export default tseslint.config(
  {
    ignores: [
      "**/dist/",
      ".meta-updater/",
      "eslint.config.mjs",
      "vitest.config.ts",
      "vitest.workspace.ts",
      "coverage/",
      "packages/app-website/.astro/",
      "packages/app-website/astro.config.mjs",
      "packages/app-website/src/env.d.ts",
      "packages/meta-api-extractor/",
      "packages/meta-eslint-config/",
      "packages/meta-eslint-plugin/lib/",
    ],
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommendedTypeChecked,
  ...tseslint.configs.strictTypeChecked,
  ...tseslint.configs.stylisticTypeChecked,
  eslintPluginUnicorn.configs["flat/recommended"],
  eslintPluginComments.recommended,
  eslintPluginDassie.configs.recommended,
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

      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["node:*"],
              message:
                "Node.js imports are only allowed in files that are marked as backend files",
            },
          ],
        },
      ],

      "@eslint-community/eslint-comments/disable-enable-pair": [
        "error",
        { allowWholeFile: true },
      ],
    },
    languageOptions: {
      ecmaVersion: 2021,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: path.resolve(import.meta.dirname, "../../"),
      },
      globals: {
        ...globals["shared-node-browser"],
      },
    },
  },

  // Frontend
  ...[
    eslintPluginReact.configs.flat.recommended,
    eslintPluginReact.configs.flat["jsx-runtime"],
    {
      plugins: { "react-hooks": eslintPluginReactHooks },
      rules: eslintPluginReactHooks.configs.recommended.rules,
    },
    {
      rules: {
        "react/prop-types": "off",
      },

      languageOptions: {
        globals: {
          ...globals.browser,
        },
      },

      settings: {
        react: {
          version: "18.3.1",
        },
      },
    },
  ].map((config) => ({
    ...config,
    files: FRONTEND_FILES,
  })),

  // Backend
  ...[
    eslintPluginN.configs["flat/recommended-module"],
    {
      rules: {
        "n/no-missing-import": "off",
        "n/no-unpublished-import": "off",
        "n/no-extraneous-import": "off",
        "n/shebang": "off",
        "n/no-process-exit": "off",

        "no-restricted-imports": "off",
      },

      languageOptions: {
        globals: {
          ...globals.node,
        },
      },
    },
  ].map((config) => ({
    ...config,
    files: BACKEND_FILES,
  })),

  // Tests
  {
    files: ["**/test/**", "**/*.test.ts{,x}"],
    rules: {
      "unicorn/consistent-function-scoping": "off",
      "@dassie/no-top-level-side-effects": "off",
      "@typescript-eslint/no-empty-function": "off",
      "@typescript-eslint/no-confusing-void-expression": "off",
      "no-restricted-imports": "off",
    },
  },

  // Examples
  {
    files: ["**/examples/**"],
    rules: {
      "no-restricted-imports": "off",
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
    files: ["next-env.d.ts"],
    rules: {
      "unicorn/prevent-abbreviations": "off",
    },
  },
  {
    files: ["**/examples/**", "**/bin/**"],
    rules: {
      "@dassie/no-top-level-side-effects": "off",
    },
  },
  eslintConfigPrettier,
)
