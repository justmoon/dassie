/**
 * Route matching for the wallet's own small router: no router of Solid 2's
 * release candidate matches the version the wallet pins.
 *
 * A pattern is a path of segments: a literal, `:name` for one segment, or
 * `*name` as the last segment for the rest of the path, slashes included.
 * A trailing slash does not matter. Parameters are URL-decoded.
 */

/** A route: a pattern and what it shows. */
export interface RouteDef<C> {
  path: string;
  component: C;
}

/** A route's parameters, by name. */
export type Params = Record<string, string>;

/** Splits a path into its segments, ignoring empty ones. */
function segments(path: string): string[] {
  return path.split("/").filter((segment) => segment.length > 0);
}

function decode(segment: string): string | undefined {
  try {
    return decodeURIComponent(segment);
  } catch {
    return undefined;
  }
}

/** The parameters of `pathname` under `pattern`, if it matches. */
export function match(pattern: string, pathname: string): Params | undefined {
  const want = segments(pattern);
  const have = segments(pathname);
  const params: Params = {};
  for (let index = 0; index < want.length; index++) {
    const part = want[index];
    if (part.startsWith("*")) {
      const rest = have.slice(index).map(decode);
      if (rest.some((segment) => segment === undefined)) return undefined;
      params[part.slice(1)] = rest.join("/");
      return params;
    }
    const segment = have[index];
    if (segment === undefined) return undefined;
    if (part.startsWith(":")) {
      const value = decode(segment);
      if (value === undefined) return undefined;
      params[part.slice(1)] = value;
    } else if (part !== segment) {
      return undefined;
    }
  }
  return want.length === have.length ? params : undefined;
}

/** The first route, in order, that `pathname` matches, and its parameters. */
export function matchRoutes<C>(
  routes: readonly RouteDef<C>[],
  pathname: string,
): { route: RouteDef<C>; params: Params } | undefined {
  for (const route of routes) {
    const params = match(route.path, pathname);
    if (params) return { route, params };
  }
  return undefined;
}

/** Joins a base path and a path within it. */
export function join(base: string, path: string): string {
  const joined = `${base.replace(/\/+$/, "")}/${path.replace(/^\/+/, "")}`;
  return joined.length > 1 ? joined.replace(/\/+$/, "") : joined;
}

/** `pathname` within `base`, or undefined if it lies outside. */
export function within(base: string, pathname: string): string | undefined {
  const prefix = base.replace(/\/+$/, "");
  if (prefix === "") return pathname;
  if (pathname === prefix) return "/";
  return pathname.startsWith(`${prefix}/`)
    ? pathname.slice(prefix.length)
    : undefined;
}
