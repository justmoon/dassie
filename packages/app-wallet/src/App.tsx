import type { Component } from "solid-js"

import logo from "./logo.svg"

const App: Component = () => {
  return (
    <div class="text-center">
      <header class="flex min-h-screen flex-col items-center justify-center bg-gray-700 text-lg text-white">
        <img
          src={logo}
          class="animate-[spin_20s_linear_infinite] h-[40vmin] pointer-events-none"
          alt="logo"
        />
        <p>
          Edit <code>src/App.tsx</code> and save to reload.
        </p>
        <a
          class="text-red-600"
          href="https://github.com/solidjs/solid"
          target="_blank"
          rel="noopener noreferrer"
        >
          Learn Solid
        </a>
      </header>
    </div>
  )
}

export default App
