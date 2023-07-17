ARG NODE_VERSION

FROM node:${NODE_VERSION} AS base

WORKDIR /app
ENV PNPM_HOME="/pnpm"
ENV PATH="$PNPM_HOME:$PATH"
RUN corepack enable && corepack prepare pnpm@8.6.8 --activate

FROM base

COPY pnpm-lock.yaml .
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm fetch
COPY . .
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm --recursive --filter=\!app-website install --frozen-lockfile --offline --ignore-scripts

ENTRYPOINT ["pnpm", "--filter=app-build", "build"]