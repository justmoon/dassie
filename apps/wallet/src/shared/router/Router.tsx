/**
 * The wallet's router: the History API, a location signal, and the routes
 * of `match.ts`. A `base` lets the dev dashboard mount the debug pages
 * under a node's own path.
 */

import { Dynamic, type JSX } from "@solidjs/web";
import {
  type Accessor,
  type Component,
  createContext,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  useContext,
} from "solid-js";
import {
  join,
  matchRoutes,
  type Params,
  type RouteDef,
  within,
} from "./match.ts";

/** What the router offers its subtree. */
export interface RouterState {
  /** The path within the base. */
  path: Accessor<string>;
  /** The query string, with its `?`. */
  search: Accessor<string>;
  /** Goes to `to`, a path within the base. */
  navigate(to: string, options?: { replace?: boolean }): void;
  /** The URL of a path within the base. */
  href(to: string): string;
}

const RouterContext = createContext<RouterState>();

/** The router of the subtree. */
export function useRouter(): RouterState {
  return useContext(RouterContext);
}

/** A query parameter of the current URL. */
export function useSearchParam(name: string): Accessor<string | null> {
  const router = useRouter();
  return () => new URLSearchParams(router.search()).get(name);
}

/** Follows the browser's location for its subtree. */
export function Router(
  props: { base?: string; children: JSX.Element },
): JSX.Element {
  const base = props.base ?? "";
  const read = () => ({ pathname: location.pathname, search: location.search });
  const [current, setCurrent] = createSignal(read());
  const onPop = () => setCurrent(read());
  addEventListener("popstate", onPop);
  onCleanup(() => removeEventListener("popstate", onPop));
  const state: RouterState = {
    path: () => within(base, current().pathname) ?? "/",
    search: () => current().search,
    href: (to) => join(base, to),
    navigate(to, options = {}) {
      const url = join(base, to);
      if (options.replace) history.replaceState(null, "", url);
      else history.pushState(null, "", url);
      setCurrent(read());
      scrollTo(0, 0);
      // Screen readers hear the new page's title.
      queueMicrotask(() =>
        document.querySelector<HTMLElement>("#content h1")?.focus()
      );
    },
  };
  return <RouterContext value={state}>{props.children}</RouterContext>;
}

/** What a route's component gets. */
export interface RouteProps {
  params: Params;
}

/** Shows the first route that matches the path, or `fallback`. */
export function Routes(props: {
  routes: readonly RouteDef<Component<RouteProps>>[];
  fallback: Component<RouteProps>;
}): JSX.Element {
  const router = useRouter();
  const matched = createMemo(() => matchRoutes(props.routes, router.path()));
  return (
    <Dynamic
      component={matched()?.route.component ?? props.fallback}
      params={matched()?.params ?? {}}
    />
  );
}

/**
 * A link that the router follows without a page load. Other clicks (with a
 * modifier, or the middle button) are the browser's.
 */
export function Link(props: {
  href: string;
  class?: string;
  /** Marks the link current only on its exact path, not below it. */
  end?: boolean;
  testid?: string;
  "aria-label"?: string;
  children: JSX.Element;
}): JSX.Element {
  const router = useRouter();
  const current = () => {
    const path = router.path();
    return props.end || props.href === "/"
      ? path === props.href
      : path === props.href || path.startsWith(`${props.href}/`);
  };
  return (
    <a
      href={router.href(props.href)}
      class={props.class}
      data-testid={props.testid}
      aria-label={props["aria-label"]}
      aria-current={current() ? "page" : undefined}
      onClick={(event: MouseEvent) => {
        if (
          event.defaultPrevented || event.button !== 0 || event.metaKey ||
          event.ctrlKey || event.shiftKey || event.altKey
        ) return;
        event.preventDefault();
        router.navigate(props.href);
      }}
    >
      {props.children}
    </a>
  );
}

/** Goes to `href` as soon as it is shown, replacing the history entry. */
export function Navigate(props: { href: string }): JSX.Element {
  const router = useRouter();
  createEffect(() => props.href, (href) => {
    router.navigate(href, { replace: true });
  });
  return null;
}
