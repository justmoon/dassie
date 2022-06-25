import { ExtractorConfig } from "@microsoft/api-extractor"
import type { ExtractorLogLevel } from "@microsoft/api-extractor"
import type { EnumMemberOrder } from "@microsoft/api-extractor-model"

/**
 * Config file for API Extractor.  For more info, please visit: https://api-extractor.com
 *
 * @param unscopedPackageName - The name of the package whose API will be extracted
 * @param entryPoint - The entry point of the package. Path relative to the package root with no leading slash and no file extension.
 * @param exportPath - The filename of the exported type rollup. Path relative to the package root with no leading slash.
 */
const createExtractorConfig = (
  unscopedPackageName: string,
  entryPoint = "src/index",
  dtsRollupPath = "dist/index.d.ts"
) =>
  ExtractorConfig.prepare({
    configObjectFullPath: new URL("../../api-extractor.json", import.meta.url)
      .pathname,
    packageJsonFullPath: new URL(
      `../../packages/${unscopedPackageName}/package.json`,
      import.meta.url
    ).pathname,
    configObject: {
      /**
       * Optionally specifies another JSON config file that this file extends from.  This provides a way for
       * standard settings to be shared across multiple projects.
       *
       * If the path starts with "./" or "../", the path is resolved relative to the folder of the file that contains
       * the "extends" field.  Otherwise, the first path segment is interpreted as an NPM package name, and will be
       * resolved using NodeJS require().
       *
       * SUPPORTED TOKENS: none
       * DEFAULT VALUE: ""
       */
      // "extends": "./shared/api-extractor-base.json"
      // "extends": "my-package/include/api-extractor-base.json"

      /**
       * Determines the "<projectFolder>" token that can be used with other config file settings.  The project folder
       * typically contains the tsconfig.json and package.json config files, but the path is user-defined.
       *
       * The path is resolved relative to the folder of the config file that contains the setting.
       *
       * The default value for "projectFolder" is the token "<lookup>", which means the folder is determined by traversing
       * parent folders, starting from the folder containing api-extractor.json, and stopping at the first folder
       * that contains a tsconfig.json file.  If a tsconfig.json file cannot be found in this way, then an error
       * will be reported.
       *
       * SUPPORTED TOKENS: <lookup>
       * DEFAULT VALUE: "<lookup>"
       */
      projectFolder: new URL("../..", import.meta.url).pathname,

      /**
       * (REQUIRED) Specifies the .d.ts file to be used as the starting point for analysis.  API Extractor
       * analyzes the symbols exported by this module.
       *
       * The file extension must be ".d.ts" and not ".ts".
       *
       * The path is resolved relative to the folder of the config file that contains the setting; to change this,
       * prepend a folder token such as "<projectFolder>".
       *
       * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
       */
      mainEntryPointFilePath: `<projectFolder>/dist/packages/${unscopedPackageName}/${entryPoint}.d.ts`,

      /**
       * A list of NPM package names whose exports should be treated as part of this package.
       *
       * For example, suppose that Webpack is used to generate a distributed bundle for the project "library1",
       * and another NPM package "library2" is embedded in this bundle.  Some types from library2 may become part
       * of the exported API for library1, but by default API Extractor would generate a .d.ts rollup that explicitly
       * imports library2.  To avoid this, we can specify:
       *
       *   "bundledPackages": [ "library2" ],
       *
       * This would direct API Extractor to embed those types directly in the .d.ts rollup, as if they had been
       * local files for library1.
       */
      bundledPackages: [],

      enumMemberOrder: "preserve" as EnumMemberOrder,

      /**
       * Determines how the TypeScript compiler engine will be invoked by API Extractor.
       */
      compiler: {
        /**
         * Specifies the path to the tsconfig.json file to be used by API Extractor when analyzing the project.
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * Note: This setting will be ignored if "overrideTsconfig" is used.
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: "<projectFolder>/tsconfig.json"
         */
        tsconfigFilePath: "<projectFolder>/tsconfig.json",
        /**
         * Provides a compiler configuration that will be used instead of reading the tsconfig.json file from disk.
         * The object must conform to the TypeScript tsconfig schema:
         *
         * http://json.schemastore.org/tsconfig
         *
         * If omitted, then the tsconfig.json file will be read from the "projectFolder".
         *
         * DEFAULT VALUE: no overrideTsconfig section
         */
        // "overrideTsconfig": {
        //   . . .
        // }
        /**
         * This option causes the compiler to be invoked with the --skipLibCheck option. This option is not recommended
         * and may cause API Extractor to produce incomplete or incorrect declarations, but it may be required when
         * dependencies contain declarations that are incompatible with the TypeScript engine that API Extractor uses
         * for its analysis.  Where possible, the underlying issue should be fixed rather than relying on skipLibCheck.
         *
         * DEFAULT VALUE: false
         */
        // "skipLibCheck": true,
      },

      /**
       * Configures how the API report file (*.api.md) will be generated.
       */
      apiReport: {
        /**
         * (REQUIRED) Whether to generate an API report.
         */
        enabled: true,

        /**
         * The filename for the API report files.  It will be combined with "reportFolder" or "reportTempFolder" to produce
         * a full file path.
         *
         * The file extension should be ".api.md", and the string should not contain a path separator such as "\" or "/".
         *
         * SUPPORTED TOKENS: <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: "<unscopedPackageName>.api.md"
         */
        reportFileName: "<unscopedPackageName>.api.md",

        /**
         * Specifies the folder where the API report file is written.  The file name portion is determined by
         * the "reportFileName" setting.
         *
         * The API report file is normally tracked by Git.  Changes to it can be used to trigger a branch policy,
         * e.g. for an API review.
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: "<projectFolder>/temp/"
         */
        reportFolder: "<projectFolder>/common/review/api/",

        /**
         * Specifies the folder where the temporary report file is written.  The file name portion is determined by
         * the "reportFileName" setting.
         *
         * After the temporary file is written to disk, it is compared with the file in the "reportFolder".
         * If they are different, a production build will fail.
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: "<projectFolder>/temp/"
         */
        reportTempFolder: "<projectFolder>/node_modules/.api-extractor/",
      },

      /**
       * Configures how the doc model file (*.api.json) will be generated.
       */
      docModel: {
        /**
         * (REQUIRED) Whether to generate a doc model file.
         */
        enabled: false,

        /**
         * The output path for the doc model file.  The file extension should be ".api.json".
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: "<projectFolder>/temp/<unscopedPackageName>.api.json"
         */
        apiJsonFilePath:
          "<projectFolder>/dist/api/<unscopedPackageName>.api.json",
      },

      /**
       * Configures how the .d.ts rollup file will be generated.
       */
      dtsRollup: {
        /**
         * (REQUIRED) Whether to generate the .d.ts rollup file.
         */
        enabled: true,

        /**
         * Specifies the output path for a .d.ts rollup file to be generated without any trimming.
         * This file will include all declarations that are exported by the main entry point.
         *
         * If the path is an empty string, then this file will not be written.
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: "<projectFolder>/dist/<unscopedPackageName>.d.ts"
         */
        untrimmedFilePath: `<projectFolder>/packages/<unscopedPackageName>/${dtsRollupPath}`,

        /**
         * Specifies the output path for a .d.ts rollup file to be generated with trimming for an "alpha" release.
         * This file will include only declarations that are marked as "\@public", "\@beta", or "\@alpha".
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: ""
         */
        // "alphaTrimmedFilePath": "<projectFolder>/dist/<unscopedPackageName>-alpha.d.ts",

        /**
         * Specifies the output path for a .d.ts rollup file to be generated with trimming for a "beta" release.
         * This file will include only declarations that are marked as "\@public" or "\@beta".
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: ""
         */
        // "betaTrimmedFilePath": "<projectFolder>/dist/<unscopedPackageName>-beta.d.ts",

        /**
         * Specifies the output path for a .d.ts rollup file to be generated with trimming for a "public" release.
         * This file will include only declarations that are marked as "\@public".
         *
         * If the path is an empty string, then this file will not be written.
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: ""
         */
        // "publicTrimmedFilePath": "<projectFolder>/dist/<unscopedPackageName>-public.d.ts",

        /**
         * When a declaration is trimmed, by default it will be replaced by a code comment such as
         * "Excluded from this release type: exampleMember".  Set "omitTrimmingComments" to true to remove the
         * declaration completely.
         *
         * DEFAULT VALUE: false
         */
        // "omitTrimmingComments": true
      },

      /**
       * Configures how the tsdoc-metadata.json file will be generated.
       */
      tsdocMetadata: {
        /**
         * Whether to generate the tsdoc-metadata.json file.
         *
         * DEFAULT VALUE: true
         */
        enabled: false,
        /**
         * Specifies where the TSDoc metadata file should be written.
         *
         * The path is resolved relative to the folder of the config file that contains the setting; to change this,
         * prepend a folder token such as "<projectFolder>".
         *
         * The default value is "<lookup>", which causes the path to be automatically inferred from the "tsdocMetadata",
         * "typings" or "main" fields of the project's package.json.  If none of these fields are set, the lookup
         * falls back to "tsdoc-metadata.json" in the package folder.
         *
         * SUPPORTED TOKENS: <projectFolder>, <packageName>, <unscopedPackageName>
         * DEFAULT VALUE: "<lookup>"
         */
        // "tsdocMetadataFilePath": "<projectFolder>/dist/tsdoc-metadata.json"
      },

      /**
       * Specifies what type of newlines API Extractor should use when writing output files.  By default, the output files
       * will be written with Windows-style newlines.  To use POSIX-style newlines, specify "lf" instead.
       * To use the OS's default newline kind, specify "os".
       *
       * DEFAULT VALUE: "crlf"
       */
      // "newlineKind": "crlf",

      /**
       * Configures how API Extractor reports error and warning messages produced during analysis.
       *
       * There are three sources of messages:  compiler messages, API Extractor messages, and TSDoc messages.
       */
      messages: {
        /**
         * Configures handling of diagnostic messages reported by the TypeScript compiler engine while analyzing
         * the input .d.ts files.
         *
         * TypeScript message identifiers start with "TS" followed by an integer.  For example: "TS2551"
         *
         * DEFAULT VALUE:  A single "default" entry with logLevel=warning.
         */
        compilerMessageReporting: {
          /**
           * Configures the default routing for messages that don't match an explicit rule in this table.
           */
          default: {
            /**
             * Specifies whether the message should be written to the the tool's output log.  Note that
             * the "addToApiReportFile" property may supersede this option.
             *
             * Possible values: "error", "warning", "none"
             *
             * Errors cause the build to fail and return a nonzero exit code.  Warnings cause a production build fail
             * and return a nonzero exit code.  For a non-production build (e.g. when "api-extractor run" includes
             * the "--local" option), the warning is displayed but the build will not fail.
             *
             * DEFAULT VALUE: "warning"
             */
            logLevel: "warning" as ExtractorLogLevel,

            /**
             * When addToApiReportFile is true:  If API Extractor is configured to write an API report file (.api.md),
             * then the message will be written inside that file; otherwise, the message is instead logged according to
             * the "logLevel" option.
             *
             * DEFAULT VALUE: false
             */
            // "addToApiReportFile": false
          },

          // "TS2551": {
          //   "logLevel": "warning",
          //   "addToApiReportFile": true
          // },
          //
          // . . .
        },

        /**
         * Configures handling of messages reported by API Extractor during its analysis.
         *
         * API Extractor message identifiers start with "ae-".  For example: "ae-extra-release-tag"
         *
         * DEFAULT VALUE: See api-extractor-defaults.json for the complete table of extractorMessageReporting mappings
         */
        extractorMessageReporting: {
          default: {
            logLevel: "warning" as ExtractorLogLevel,
            // "addToApiReportFile": false
          },

          // "ae-extra-release-tag": {
          //   "logLevel": "warning",
          //   "addToApiReportFile": true
          // },
          //
          // . . .
        },

        /**
         * Configures handling of messages reported by the TSDoc parser when analyzing code comments.
         *
         * TSDoc message identifiers start with "tsdoc-".  For example: "tsdoc-link-tag-unescaped-text"
         *
         * DEFAULT VALUE:  A single "default" entry with logLevel=warning.
         */
        tsdocMessageReporting: {
          default: {
            logLevel: "warning" as ExtractorLogLevel,
            // "addToApiReportFile": false
          },

          // "tsdoc-link-tag-unescaped-text": {
          //   "logLevel": "warning",
          //   "addToApiReportFile": true
          // },
          //
          // . . .
        },
      },
    },
  })

export default createExtractorConfig
