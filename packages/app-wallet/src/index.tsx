/* @refresh reload */
import { render } from "solid-js/web"
import Uplink from "./modules/uplink"

import "./index.css"
import App from "./App"

render(() => <App />, document.getElementById("root") as HTMLElement)

new Uplink()
