declare module "launch-editor" {
  const launchEditor: (
    location: string,
    editor?: string,
    callback?: () => void,
  ) => void
  export = launchEditor
}
