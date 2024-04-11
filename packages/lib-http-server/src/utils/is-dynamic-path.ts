export function isDynamicPath(path: string[]): boolean {
  return path.some((segment) => segment.startsWith(":") || segment === "*")
}
