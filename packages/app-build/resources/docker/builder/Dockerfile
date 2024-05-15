ARG NODE_VERSION

FROM node:${NODE_VERSION} AS base

WORKDIR /app

ADD packages/app-build/resources/binaries/binary-dependencies.txt .

ENV DASSIE_BUILD_BINARY_DEPENDENCIES_PATH="/opt/binaries"
RUN set -xe \
  && mkdir -p "$DASSIE_BUILD_BINARY_DEPENDENCIES_PATH" \
  && cd "$DASSIE_BUILD_BINARY_DEPENDENCIES_PATH" \
  && while IFS=" " read -r url hash; do \
       filename="$(basename "$url")"; \
       curl -L -o "$filename" "$url"; \
       echo "$hash $filename" | sha256sum -c -; \
     done < /app/binary-dependencies.txt

ENV PNPM_HOME="/pnpm"
ENV PATH="$PNPM_HOME:$PATH"

ADD package.json .
RUN corepack enable && corepack prepare --activate

FROM base

COPY pnpm-lock.yaml .
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm fetch
COPY . .
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm --recursive --filter=\!app-website install --frozen-lockfile --offline --ignore-scripts

ENTRYPOINT ["pnpm", "--filter=app-build", "build"]