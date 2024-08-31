# lib-reactive

## What is this?

It's a TypeScript framework which is designed to run in both front- and backend code and synchronize between them. It loosely follows the actor model architecture found in Erlang and Akka. It also borrows ideas from reactive programming UI frameworks like React, Solid.js, and [Reactively](https://github.com/modderme123/reactively).

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
const RootActor = () => createActor(() => console.log("Hello world"))

createReactor(RootActor)
```

You can think of the actor as a stateful entity which can do things when it is created, destroyed, or when it receives a message.

### Instances and context

You may have noticed in the previous example that `RootActor` wasn't just an instance of an actor, but actually a `Factory` function that creates that type of actor. The reactor takes care of instantiating any value we ask for using the `reactor.use` method. If the value has already been instantiated, it is returned. If it hasn't been, then it is created first.

By convention, variables representing factories are always camel-case (e.g. `RootActor`) and variables containing the actual instance have the same name but in mixed-case (e.g. `rootActor`).

```ts
const RootActor = () => createActor(() => console.log("Hello world"))

const reactor = createReactor(RootActor)

const rootActor = reactor.use(RootActor)
```

Factories don't always have to return actors, they can return any JavaScript value. We'll soon learn about some other common types of values, such as topics and signals.

So why do we use factories instead of just defining our values globally? There are a number of benefits

- The main benefit is that it allows each instance of the actor or the topic to be scoped to a specific `Reactor`. In other words, when we publish a message on a given topic in one reactor, the message will not appear in any other reactor. This allows us to run multiple instances of our application in the same JavaScript context such as a Node.js process or browser tab. As a result, it is much more efficient for example to run tests in parallel or simulate distributed applications.

- It also means that everything is initialized lazily, i.e. the topic is only instantiated if it is actually being used.

- Functions have a unique property in JavaScript: They capture their own name when they are being defined. In the example above, `RootActor.name` will be set to `"RootActor"`. We can use that feature to provide better error messages and such without any additional boilerplate like having to give things a name.

- Values can be manually overridden by calling `reactor.set()`. This allows for very easy mocking during testing.

### Actor hierarchy

> Actors all the way down!

The reactor will pass an `ActorContext` to the actor as the first parameter. This is how the actor interacts with its environment. For example, we can use it to instantiate subsidiary actors with the method `run`.

```ts
const RootActor = () => createActor((sig) =>
  console.log("Hello from the root actor")

  sig.run(subActor)
})

const SubActor = () => createActor((sig) =>
  console.log("Hello from the sub actor")
})

createReactor(rootActor)
```

Overall, actors form a hierarchy which allows you to structure your application. This is a very common pattern, so you're probably familiar with it. Frontend frameworks like React for example structure applications using a hierarchy of components.

### First encounter with scope

> All good things must come to an end!

Actors have a lifecycle - they are created and disposed. The root actor for instance is disposed when we dispose the reactor.

```ts
const RootActor = () =>
  createActor((sig) => {
    const interval = setInterval(() => console.log("tick!"), 100)

    sig.onCleanup(() => clearInterval(interval))
  })

const reactor = createReactor(RootActor)

setTimeout(() => reactor.dispose(), 1000)
```

Calling `sig.onCleanup` registers a callback that we want to execute when the current actor is being disposed. So in the code above, the interval will be disposed when `reactor.dispose()` is called.

It's very common pattern that actors require some cleanup. If the actors is listening to an event, you'll want to unregister that event handler when the actors is disposed. If the actors is starting a server, you may want to shut that server down, etc.

We can actually simplify the code above by using `sig.interval` which is a built-in helper that automatically disposes the interval for us. When we are using any of the `sig.*` helper methods, the disposal is always handled for us automatically.

```ts
const RootActor = () =>
  createActor((sig) => {
    sig.interval(() => console.log("tick!"), 100)
  })

const reactor = createReactor(RootActor)

setTimeout(() => reactor.dispose(), 1000)
```

Notice that we we're still using `setTimeout` and we're not cleaning up that timeout. That's fine in this case but what if there were multiple ways for our application to finish?

```ts
const RootActor = () =>
  createActor((sig) => {
    sig.interval(() => console.log("tick!"), 100)
  })

const reactor = createReactor(RootActor)

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
const RootActor = () =>
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

createReactor(RootActor)
```

Now, when the first shutdown timer is hit, it will dispose of the reactor which will automatically clean up the other one.

### Messaging

> This is where it gets interesting.

So far, we've looked at how we can define the basic structure of our application using actors. However, currently, none of these actors can communicate with each other. Fortunately, doing so is very simple.

```ts
const RootActor = () =>
  createActor((sig) => {
    // Instantiate the two actors
    sig.run(PingActor)
    sig.run(PongActor)

    // Get the process started by sending a message to the ping actor
    sig.reactor.use(PingActor).api.ping.tell()
  })

const PingActor = () =>
  createActor((sig) => {
    const pongActor = sig.reactor.use(PongActor)
    return {
      ping: () => pongActor.api.pong.tell(),
    }
  })

const PongActor = () =>
  createActor((sig) => {
    const pingActor = sig.reactor.use(PingActor)
    return {
      pong: () => {
        sig.timeout(() => {
          pingActor.api.ping.tell()
        }, 75)
      },
    }
  })

createReactor(RootActor)
```

In this example, the root actor will instantiate two actors who will send each other messages back and forth indefinitely.

### Topics

> Everybody always wants to talk, talk, talk

We just learned how one actor can communicate with other other actor. But sometimes we want multiple actors to communicate with multiple other actors. We can achieve this using topics.

To define a new Topic, we need to make a factory function that calls `createTopic`.

```ts
const PingPongTopic = () => createTopic<string>()
```

In order to `emit` a message on a topic, we first need to get an instance of the topic. In order to do that, we can call `sig.reactor.use` from inside of an actor:

```ts
const RootActor = () =>
  createActor((sig) => {
    sig.reactor.use(PingPongTopic).emit("ping")
  })

createReactor(RootActor)
```

Obviously, emitting events is not very useful when nobody is listening. So let's listen using `on`.

```ts
const RootActor = () =>
  createActor((sig) => {
    const dispose = sig.on(PingPongTopic, (message) => {
      console.log(message)
    })

    sig.onCleanup(dispose)
  })

createReactor(rootActor)
```

When we create listeners manually via `sig.reactor.use().on()` we also have to remember to dispose of them using `sig.onCleanup`. That could get tedious quickly. Instead, we can use the `sig.on` shorthand which will handle the cleanup for us automatically.

```ts
const RootActor = () =>
  createActor((sig) => {
    sig.on(PingPongTopic, (message) => {
      console.log(message)
    })
  })

createReactor(RootActor)
```

Ok, now let's put all of these pieces together and look at a complete example:

```ts
import {
  EffectContext,
  createActor,
  createReactor,
  createTopic,
} from "@dassie/lib-reactive"

const PingPongTopic = () => createTopic<string>()

const Pinger = () =>
  createActor((sig) => {
    sig.on(PingPongTopic, (message) => {
      if (message === "pong") {
        sig.reactor.use(PingPongTopic).emit("ping")
      }
    })
  })

const Ponger = () =>
  createActor((sig) => {
    sig.on(PingPongTopic, (message) => {
      if (message === "ping") {
        sig.timeout(() => {
          sig.reactor.use(PingPongTopic).emit("pong")
        }, 75)
      }
    })
  })

const Logger = () =>
  createActor((sig) => {
    sig.on(PingPongTopic, console.log)
  })

const RootActor = () =>
  createActor((sig) => {
    sig.run(Pinger)
    sig.run(Ponger)
    sig.run(Logger)
    sig.emit(PingPongTopic, "ping")
    sig.timeout(() => sig.reactor.dispose(), 200)
  })

createReactor(RootActor)
```

There are three actors, `Pinger`, `Ponger`, and `Logger`. Pinger will watch the `PingPongTopic` and if it sees a `"pong"` message emit a `"ping"` message. Ponger will emit a `"pong"` message 75 milliseconds after it sees a `"ping"` message. Logger will simply log these messages to the console.

### Signals

> I'll never forget this!

Although actors can keep state internally, they are intended to be disposable, meaning they should be able to be torn down and recreated at any time. That means we need some place to keep the state of our application.

The solution is a different kind of value called a signal. Signals provide the methods `read` and `write` which allows you access and modify their internal state. You can also call `update` and pass a reducer which accepts the previous state and returns a new state. When creating a new signal, you can pass an `initialValue`.

Actors can access signals using a special helper method `sig.readAndTrack` which will tracked which signals the actor accessed and restart the actor if any of these signals' values change.

Let's see an example.

```ts
import {
  EffectContext,
  createActor,
  createReactor,
  createSignal,
} from "@dassie/lib-reactive"

const CounterSignal = () => createSignal(0)

const Clock = () =>
  createActor((sig) => {
    sig.interval(() => {
      sig.reactor.use(CounterSignal).update((state) => state + 1)
    }, 75)
  })

const Logger = () =>
  createActor((sig) => {
    const counterValue = sig.readAndTrack(CounterSignal)

    console.log(`the counter is: ${counterValue}`)
  })

const RootActor = () =>
  createActor((sig) => {
    sig.run(Clock)
    sig.run(Logger)
    sig.timeout(() => void sig.reactor.dispose(), 400)
  })

createReactor(RootActor)
```

### Signals as topics

Signals also expose the topic API meaning you can subscribe to changes using `on` and `once`.

That means another way we could have implemented the Logger actor from the previous example is like this:

```ts
import {
  EffectContext,
  createActor,
  createReactor,
  createSignal,
} from "@dassie/lib-reactive"

const CounterSignal = () => createSignal(0)

const Logger = () =>
  createActor((sig) => {
    sig.on(CounterSignal, (counterValue) => {
      console.log(`the counter is: ${counterValue}`)
    })
  })
```

Usually, tracked reads (using `sig.readAndTrack`) are the preferred method for watching a signal. But tracked reads are only
available in certain contexts (like in the behavior function of an actor) so it is sometimes useful to be able to treat
signals as topics.

Note that when a signal is updated multiple times in the same tick it will only emit the latest value. If you need to
keep track of every change, publish the changes to a topic instead.

### Computed values

Sometimes we may have values that are computed but need to be updated whenever the underlying state changes. This can be represented using `Computed` values. These values are themselves signals which can be watched by other computed values
as well as actors.

```ts
import {
  EffectContext,
  createActor,
  createComputed,
  createReactor,
  createSignal,
} from "@dassie/lib-reactive"

const CounterSignal = () => createSignal(0)

const DoubledComputed = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    return sig.readAndTrack(CounterSignal) * 2
  })

const Clock = () =>
  createActor((sig) => {
    sig.interval(() => {
      sig.reactor.use(CounterSignal).update((state) => state + 1)
    }, 75)
  })

const Logger = () =>
  createActor((sig) => {
    const counterValue = sig.readAndTrack(CounterSignal)
    const doubledValue = sig.readAndTrack(DoubledComputed)

    console.log(`the counter value is: ${counterValue}`)
    console.log(`the doubled value is: ${doubledValue}`)
  }

const RootActor = () =>
  createActor((sig) => {
    sig.run(Clock)
    sig.run(Logger)
    sig.timeout(() => void sig.reactor.dispose(), 400)
  })

createReactor(RootActor))
```

Computed values are executed lazily by default, meaning they are only actually calculated when the computed value (or
one of its dependent computed values) is actually read. There are some exceptions when computed values will become
eagerly calculated:

- When they are accessed from an actor using `sig.readAndTrack`
- When they are subscribed to as a topic using `on` or `once`

### Batching

When multiple signals update at the same time, any dependent values will only be recomputed once.

There is a special `sig.readAndTrack` helper which will retrieve the current state of a signal but also listen for changes and automatically re-run the actor with the new value. This allows us to build some very concise reactive applications.

```ts
import {
  EffectContext,
  createActor,
  createReactor,
  createTopic,
} from "@dassie/lib-reactive"

const Signal1 = () => createSignal(0)
const Signal2 = () => createSignal(0)
const Signal3 = () => createSignal(0)

const Logger = () =>
  createActor((sig) => {
    const t1 = sig.readAndTrack(Signal1)
    const t2 = sig.readAndTrack(Signal2)
    const t3 = sig.readAndTrack(Signal3)

    console.log(`actor run with ${t1} ${t2} ${t3}`)
  })

const RootActor = () =>
  createActor((sig) => {
    sig.interval(() => {
      // Even though we are triggering three state updates, the actor will only re-run once
      sig.reactor.use(Signal1).update((a) => a + 1)
      sig.reactor.use(Signal2).update((a) => a + 3)
      sig.reactor.use(Signal3).update((a) => a + 5)
    }, 1000)

    sig.run(Logger)

    // Stop the application after 10 seconds
    sig.timeout(sig.reactor.dispose, 10_000)
  })

createReactor(RootActor)
```
