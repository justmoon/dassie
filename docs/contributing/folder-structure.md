# Dassie Repository Directory Structure

## Packages

Dassie is a monorepo consisting of packages. We are using pnpm as the package manager.

Packages are marked with one of a number of prefixes depending on their type:

- **app-** - Applications, i.e. code that you can directly execute
- **lib-** - Libraries, i.e. code that is meant to be included by other code
- **meta-** - Meta-packages are those which are used by the development environment itself, such as eslint configuration etc.

## Features

The more complex pieces of the Dassie codebase are broken down into features. For example, the Dassie node backend is found in `packages/app-node/src/backend/` and contains features like `accounting`, `authentication`, and `ilp-connector`.

Within each feature, you may find various subfolders containing different types of code assets:

- **actors/** - Actors using `createActor` from `@dassie/lib-reactive` (Note: Currently, actors are often just on the top-level directory of the feature)
- **computed/** - Derived state using `createComputed` from `@dassie/lib-reactive`
- **constants/** - Any constant values used by the feature
- **database-stores/** - Similar to `stores/` but specifically stores that are backed by a database table
- **database-tables/** - Database tables using `table` from `@dassie/lib-sqlite`
- **failures/** - Classes representing expected failure results that can be returned in a type-safe way using `Failure` from `@dassie/lib-type-utils`
- **functions/** - Functions which rely on some context values and therefore need to be instantiated with a specific Reactor (see [/packages/lib-reactive/README.md]())
- **signals/** - Stateful signals using `createSignal` from `@dassie/lib-reactive`
- **stores/** - State stores using `createStore` from `@dassie/lib-reactive`
- **topics/** - Any publish/subscribe topics using `createTopic` from `@dassie/lib-reactive`
- **trpc-routers/** - Any APIs that are exposed to the frontend or the CLI
- **types/** - TypeScript type definitions that are relevant to this feature
- **utils/** - Stateless utility functions

These are the common directory names inside features. Some features may have specific subfolders that are unique to that feature. For example, `settlement-schemes/` contains a `modules/` folder which in turn contains all of the different settlement scheme modules.
