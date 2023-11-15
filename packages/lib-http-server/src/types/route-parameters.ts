export interface ParametersDictionary {
  [key: string]: string
}

type RemoveTail<
  S extends string,
  Tail extends string,
> = S extends `${infer P}${Tail}` ? P : S
type GetRouteParameter<S extends string> = RemoveTail<
  RemoveTail<RemoveTail<S, `/${string}`>, `-${string}`>,
  `.${string}`
>

export type RouteParameters<Route extends string> = string extends Route
  ? ParametersDictionary
  : Route extends `${string}(${string}`
  ? ParametersDictionary // no handling for regex routes
  : Route extends `${string}:${infer Rest}`
  ? (GetRouteParameter<Rest> extends never
      ? ParametersDictionary
      : GetRouteParameter<Rest> extends `${infer ParameterName}?`
      ? { [P in ParameterName]?: string }
      : { [P in GetRouteParameter<Rest>]: string }) &
      (Rest extends `${GetRouteParameter<Rest>}${infer Next}`
        ? RouteParameters<Next>
        : unknown)
  : object
