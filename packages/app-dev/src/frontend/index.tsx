/* @refresh reload */
import "@unocss/reset/tailwind.css"
import { Router } from "solid-app-router"
import { render } from "solid-js/web"
import "uno.css"
import "virtual:unocss-devtools"

import { assertDefined } from "@xen-ilp/lib-type-utils"

import App from "./app"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
render(
  () => (
    <Router>
      <App />
    </Router>
  ),
  rootElement
)
