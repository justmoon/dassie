# lib-reactive

## What is this?

It's a TypeScript framework which is designed to run in both front- and backend code and synchronize between them. It loosely follows the actor model architecture found in Erlang and Akka. It also borrows ideas from reactive programming UI frameworks like React and Solid.js.

## Features

- Lifecycle management: Automatically clean things up when they need to be cleaned up
- Invalidation: Automatically re-do stuff when dependencies change
- Context: Get values to where they're needed
- Debugging: Get an understanding of what's happening in your system

## Getting Started

### Creating a reactor

> Hello World!

To get lib-reactive to do anything, we first need to create a reactor. A reactor is the overall context in which things happen. To create a reactor, we call `createReactor` and we pass a function that returns an actor.

```ts
const rootActor = () => createActor(() => console.log("Hello world"))

createReactor(rootActor)
```

You can think of the actor as a stateful entity which can do things when it is created, destroyed, or when it receives a message.

### Actor hierarchy

> Actors all the way down!

The reactor will pass an `ActorContext` to the actor as the first parameter. This is how the actor interacts with its environment. For example, we can use it to instantiate subsidiary actors with the method `run`.

```ts
const rootActor = () => createActor((sig) =>
  console.log("Hello from the root actor")

  sig.run(subActor)
})

const subActor = () => createActor((sig) =>
  console.log("Hello from the sub actor")
})

createReactor(rootActor)
```

Overall, actors form a hierarchy which allows you to structure your application. This is a very common pattern, so you're probably familiar with it. Frontend frameworks like React for example structure applications using a hierarchy of components.

### First encounter with lifecycles

> All good things must come to an end!

Actors have a lifecycle - they are created and disposed. The root actor for instance is disposed when we dispose the reactor.

```ts
const rootActor = () =>
  createActor((sig) => {
    const interval = setInterval(() => console.log("tick!"), 100)

    sig.onCleanup(() => clearInterval(interval))
  })

const reactor = createReactor(rootActor)

setTimeout(() => reactor.dispose(), 1000)
```

Calling `sig.onCleanup` registers a callback that we want to execute when the current actor is being disposed. So in the code above, the interval will be disposed when `reactor.dispose()` is called.

It's very common pattern that actors require some cleanup. If the actors is listening to an event, you'll want to unregister that event handler when the actors is disposed. If the actors is starting a server, you may want to shut that server down, etc.

We can actually simplify the code above by using `sig.interval` which is a built-in helper that automatically disposes the interval for us. When we are using any of the `sig.*` helper methods, the disposal is always handled for us automatically.

```ts
const rootActor = () =>
  createActor((sig) => {
    sig.interval(() => console.log("tick!"), 100)
  })

const reactor = createReactor(rootActor)

setTimeout(() => reactor.dispose(), 1000)
```

Notice that we we're still using `setTimeout` and we're not cleaning up that timeout. That's fine in this case but what if there were multiple ways for our application to finish?

```ts
const rootActor = () =>
  createActor((sig) => {
    sig.interval(() => console.log("tick!"), 100)
  })

const reactor = createReactor(rootActor)

setTimeout(() => {
  console.log("exiting after one second")
  reactor.dispose()
}, 1000)

setTimeout(() => {
  console.log("exiting after a random amount of time")
  reactor.dispose()
}, 2000 * Math.random())
```

Here, both timeouts will eventually be called and both console.log statements will eventually be executed. This is not what we want. It's a good rule of thumb that your entire application should be contained inside of your reactor. Let's refactor that last example by moving the shutdown timers into the root actor.

```ts
const rootActor = () =>
  createActor((sig) => {
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

createReactor(rootActor)
```

Now, when the first shutdown timer is hit, it will dispose of the reactor which will automatically clean up the other one.

### Messaging

> This is where it gets interesting.

So far, we've looked at how we can define the basic structure of our application using actors. However, currently, none of these actors can communicate with each other. Fortunately, doing so is very simple.

```ts
const rootActor = () => createActor((sig) => {
  // Instantiate the two actors
  sig.run(pingActor)
  sig.run(pongActor)

  // Get the process started by sending a message to the ping actor
  sig.use(pingActor).tell()
})

const pingActor = () => createActor((sig) => {
  return () => sig.use(pongActor).tell()
})

const pongActor = () => createActor((sig) => {
  return () => {
    sig.timeout(() => {
      sig.use(pingActor).tell()
    }, 75)
})
```

In this example, the root actor will instantiate two actors who will send each other messages back and forth indefinitely.

### Topics

> Everybody always wants to talk, talk, talk

We just learned how one actor can communicate with other other actor. But sometimes we want multiple actors to communicate with multiple other actors. We can achieve this using topics.

To define a new Topic, we need to make a factory function that calls `createTopic`.

```ts
const pingPongTopic = () => createTopic<string>()
```

You may have noticed before that we have been creating factory functions instead of just storing the return value of `createActor`/`createTopic` directly. So why are we doing this? There are several benefits to this approach:

- The main benefit is that it allows each instance of the actor or the topic to be scoped to a specific `Reactor`. In other words, when we publish a message on a given topic in one reactor, the message will not appear in any other reactor. This allows us to run multiple instances of our application in the same JavaScript context such as a Node.js process or browser tab. As a result, it is much more efficient for example to run tests in parallel or simulate distributed applications.

- It also means that everything is initialized lazily, i.e. the topic is only instantiated if it is actually being used.

- Functions have a unique property in JavaScript: They capture their own name when they are being defined. In the example above, `pingPongTopic.name` will be set to `"pingPongTopic"`. We can use that feature to provide great debuggability without any additional boilerplate like having to give things a name.

In order to `emit` a message on a topic, we first need to get an instance of the topic. In order to do that, we can call `sig.use` from inside of an actor:

```ts
const rootActor = () =>
  createActor((sig) => {
    sig.use(pingPongTopic).emit("ping")
  })

createReactor(rootActor)
```

Obviously, emitting events is not very useful when nobody is listening. So let's listen using `on`.

```ts
const rootActor = () =>
  createActor((sig) => {
    const dispose = sig.use(pingPongTopic).on((message) => {
      console.log(message)
    })

    sig.onCleanup(dispose)
  })

createReactor(rootActor)
```

When we create listeners manually via `sig.use().on()` we also have to remember to dispose of them using `sig.onCleanup`. That could get tedious quickly. Instead, we can use the `sig.on` shorthand which will handle the cleanup for us automatically.

```ts
const rootActor = () =>
  createActor((sig) => {
    sig.on(pingPongTopic, (message) => {
      console.log(message)
    })
  })

createReactor(rootActor)
```

Ok, now let's put all of these pieces together and look at a complete example:

```ts
import { EffectContext, createReactor, createTopic } from "@dassie/lib-reactive"

const pingPongTopic = () => createTopic<string>()

const pinger = () =>
  createActor((sig) => {
    sig.on(pingPongTopic, (message) => {
      if (message === "pong") {
        sig.emit(pingPongTopic, "ping")
      }
    })
  })

const ponger = () =>
  createActor((sig) => {
    sig.on(pingPongTopic, (message) => {
      if (message === "ping") {
        sig.timeout(() => {
          sig.use(pingPongTopic).emit("pong")
        }, 75)
      }
    })
  })

const logger = () =>
  createActor((sig) => {
    sig.on(pingPongTopic, console.log)
  })

const rootActor = () =>
  createActor((sig) => {
    sig.run(pinger)
    sig.run(ponger)
    sig.run(logger)
    sig.emit(pingPongTopic, "ping")
    sig.timeout(() => sig.reactor.dispose(), 200)
  })

createReactor(rootActor)
```

There are three actors, `pinger`, `ponger`, and `logger`. Pinger will watch the `pingPongTopic` and if it sees a `"pong"` message emit a `"ping"` message. Ponger will emit a `"pong"` message 75 milliseconds after it sees a `"ping"` message. Logger will simply log these messages to the console.

### Signals

> I'll never forget this!

Although actors can keep state internally, they are intended to be disposable, meaning they should be able to be torn down and recreated at any time.

Signals are stateful topics. They provide methods `read` and `write` which allows you access and modify their internal state. You can also call `update` and pass a reducer which accepts the previous state and returns a new state. Whenever the state changes, the signal will emit the new state so you can listen to it. When creating a new signal, you can pass an `initialValue`.

Let's see an example.

```ts
import {
  EffectContext,
  createReactor,
  createSignal,
} from "@dassie/lib-reactive"

const counterSignal = () => createSignal(0)

const clock = () =>
  createActor((sig) => {
    sig.interval(() => {
      sig.use(counterSignal).update((state) => state + 1)
    }, 75)
  })

const logger = () =>
  createActor((sig) => {
    sig.on(counterSignal, (state) => {
      console.log(`the counter is: ${state}`)
    })
  })

const rootActor = () =>
  createActor((sig) => {
    sig.run(clock)
    sig.run(logger)
    sig.timeout(() => void sig.reactor.dispose(), 400)
  })

createReactor(rootActor)
```

### Tracked access

We've seen how to listen to topics, but so far we have still had to manage these listeners manually.

There is a special `sig.get` helper which will retrieve the current state of a signal but also listen for changes and automatically re-run the actor with the new value. This allows us to build some very concise reactive applications.

```ts
import { EffectContext, createReactor, createTopic } from "@dassie/lib-reactive"

const signal1 = () => createSignal(0)
const signal2 = () => createSignal(0)
const signal3 = () => createSignal(0)

const logger = () =>
  createActor((sig) => {
    const t1 = sig.get(signal1)
    const t2 = sig.get(signal2)
    const t3 = sig.get(signal3)

    console.log(`actor run with ${t1} ${t2} ${t3}`)
  })

const rootEffect = () =>
  createActor((sig) => {
    sig.interval(() => {
      // Even though we are triggering three state updates, the actor will only re-run once
      sig.use(signal1).update((a) => a + 1)
      sig.use(signal2).update((a) => a + 3)
      sig.use(signal3).update((a) => a + 5)
    }, 1000)

    sig.run(logger)

    // Stop the application after 10 seconds
    sig.timeout(sig.reactor.dispose, 10_000)
  })

createReactor(rootEffect)
```
