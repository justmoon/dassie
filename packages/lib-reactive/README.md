# lib-reactive

## What is this?

It's a reactive state management library designed to work for both front- and back-end and synchronize between them.

## Features

- Lifecycle management: Automatically clean things up when they need to be cleaned up
- Invalidation: Automatically re-do stuff when dependencies change
- Context: Get values to where they're needed
- Debugging: Get an understanding of what's happening in your system

## Getting Started

### Creating a reactor

> Hello World!

To get lib-reactive to do anything, we first need to create a reactor. A reactor is an isolated context in which things happen. To create a reactor, we call `createReactor` and we pass an effect. An effect is simply any JavaScript function.

```ts
createReactor(() => console.log("Hello world"))
```

You can think of the effect as a description of everything that should _happen_ inside of the reactor.

### Effect hierarchy

> Effects all the way down!

The reactor will pass an `EffectContext` to the effect as the first parameter. This is how the effect interacts with the reactive context. For example, we can use it to instantiate further effects with the method `use`.

```ts
createReactor((sig) => {
  console.log("Hello from the root effect")

  sig.run(() => {
    console.log("Hello from the child effect")
  })
})
```

Overall, effects form a hierarchy which allows you to structure your application. This is a very common pattern, so you're probably familiar with it. Frontend frameworks like React structure applications using a hierarchy of components. Systems based on the actor model such as Erlang use a hierarchy of actors, and so on.

### First encounter with lifecycles

> All good effects must come to an end!

Effects have a lifecycle - they are created and disposed. The root effect for instance is disposed when we dispose the reactor.

```ts
const reactor = createReactor((sig) => {
  const interval = setInterval(() => console.log("tick!"), 100)

  sig.onCleanup(() => clearInterval(interval))
})

setTimeout(() => reactor.dispose(), 1000)
```

Calling `sig.onCleanup` registers a callback that we want to execute when the current effect is being disposed. So in the code above, the interval will be disposed when `reactor.dispose()` is called.

It's very common pattern that effects require some cleanup. If the effect is listening to an event, you'll want to unregister that event handler when the effect is disposed. If the effect is starting a server, you may want to shut that server down, etc.

We can actually simplify the code above by using `sig.interval` which is a built-in helper that automatically disposes the interval for us. When you are using any of the `sig.*` helper methods, the disposal is always handled for you automatically.

```ts
const reactor = createReactor((sig) => {
  sig.interval(() => console.log("tick!"), 100)
})

setTimeout(() => reactor.dispose(), 1000)
```

Notice that we we're still using `setTimeout` and we're not cleaning up that timeout. That's fine in this case but what if there were multiple ways for our application to finish?

```ts
const reactor = createReactor((sig) => {
  sig.interval(() => console.log("tick!"), 100)
})

setTimeout(() => {
  console.log("exiting after one second")
  reactor.dispose()
}, 1000)
setTimeout(() => {
  console.log("exiting after a random amount of time")
  reactor.dispose()
}, 2000 * Math.random())
```

Here, both timeouts will eventually be called and both console.log statements will eventually be executed. This is not what we want. It's a good rule of thumb that your entire application should be contained inside of your reactor. Let's refactor that last example by moving the shutdown timers into the reactor.

```ts
createReactor((sig) => {
  sig.interval(() => console.log("tick!"), 100)

  sig.timeout(() => {
    console.log("exiting after one second")
    reactor.dispose()
  }, 1000)

  sig.timeout(() => {
    console.log("exiting after a random amount of time")
    reactor.dispose()
  }, 2000 * Math.random())
})
```

Now, when the first shutdown timer is hit, it will dispose of the reactor which will automatically clean up the other one.

### Topics

> This is where it gets interesting.

So far, we've looked at how we can define the basic structure of our application using effects. However, currently, none of these different components can communicate with each other. We solve this using another primitive called a `Topic`.

For now, just think of a topic as a static symbol that we can reference. Effects can `emit` messages about a topic, and other effects can listen to new messages `on` a topic.

Let's see an example.

```ts
import { EffectContext, createReactor, createTopic } from "@dassie/lib-reactive"

const pingPongTopic = () => createTopic<string>()

const pinger = (sig: EffectContext) => {
  sig.on(pingPongTopic, (message) => {
    if (message === "pong") {
      sig.emit(pingPongTopic, "ping")
    }
  })
}

const ponger = (sig: EffectContext) => {
  sig.on(pingPongTopic, (message) => {
    if (message === "ping") {
      sig.timeout(() => {
        sig.emit(pingPongTopic, "pong")
      }, 75)
    }
  })
}

const logger = (sig: EffectContext) => {
  sig.on(pingPongTopic, console.log)
}

createReactor((sig: EffectContext) => {
  sig.run(pinger)
  sig.run(ponger)
  sig.run(logger)
  sig.emit(pingPongTopic, "ping")
  sig.timeout(() => sig.reactor.dispose(), 200)
})
```

There are three effects, `pinger`, `ponger`, and `logger`. Pinger will watch the `pingPongTopic` and if it sees a `"pong"` message emit a `"ping"` message. Ponger will emit a `"pong"` message 75 milliseconds after it sees a `"ping"` message. Logger will simply log these messages to the console.

### Signals

> I'll never forget this!

Signals are stateful topics. They provide methods `read` and `write` which allows you access and modify their internal state. You can also call `update` and pass a reducer which accepts the previous state and returns a new state. Whenever the state changes, the signal will emit the new state so you can listen to it. When creating a new signal, you can pass an `initialValue`.

That means that signals are effectively stateful topics. Let's see an example.

```ts
import {
  EffectContext,
  createReactor,
  createSignal,
} from "@dassie/lib-reactive"

const counterSignal = () => createSignal(0)

const clock = (sig: EffectContext) => {
  sig.interval(() => {
    sig.use(counterSignal).update((state) => state + 1)
  }, 75)
}

const logger = (sig: EffectContext) => {
  sig.on(counterSignal, (state) => {
    console.log(`the counter is: ${state}`)
  })
}

createReactor((sig: EffectContext) => {
  sig.run(clock)
  sig.run(logger)
  sig.timeout(() => void sig.reactor.dispose(), 400)
})
```

### Tracked access

We've seen how to listen to topics, but so far we have still had to manage these listeners manually.

There is a special `sig.get` helper which will receive the last value from a topic and automatically dispose and re-run the effect if this value changes. This allows us to build some very concise reactive applications.

```ts
import { EffectContext, createReactor, createTopic } from "@dassie/lib-reactive"

const signal1 = () => createSignal(0)
const signal2 = () => createSignal(0)
const signal3 = () => createSignal(0)

const rootEffect = (sig: EffectContext) => {
  sig.interval(() => {
    // Even though we are triggering three state updates, the effect will only re-run once
    sig.use(signal1).update((a) => a + 1)
    sig.use(signal2).update((a) => a + 3)
    sig.use(signal3).update((a) => a + 5)
  }, 1000)

  sig.run((sig) => {
    const t1 = sig.get(signal1)
    const t2 = sig.get(signal2)
    const t3 = sig.get(signal3)

    console.log(`effect run with ${t1} ${t2} ${t3}`)
  })

  // Stop the application after 10 seconds
  sig.timeout(sig.reactor.dispose, 10_000)
}

createReactor(rootEffect)
```

It's important to note that `sig.get` should only be called from the main body of the effect.
