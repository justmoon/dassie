# Implementation Notes: Frontend Security

## Signed frontend code

The root seed is generated client-side in the frontend code, which means that the frontend has theoretically higher privileges than the Dassie node it controls. In practice, of course, the frontend is served by the node meaning the node could simply send different code if it wanted to get access to those privileges.

However, in the future, we have two ways of making that more difficult:

### Option 1: Independent frontend

This is pretty straightforward. The frontend is a downloadable app that the user installs locally on their computer. Once installed, they enter the domain of their Dassie node to connect to it. Now the Dassie node has no control over the frontend.

In order to avoid having different versions of frontend and backend, we could still allow the backend to serve the frontend code but require a signed manifest to ensure that the frontend is the unmodified version released by the developers. The client would then just be a thin shim which would load the frontend code, verify the signature, manage some local state, prevent the node from trying to downgrade the frontend etc.

### Option 2: Service worker verification

A slightly less secure but perhaps more user-friendly option would be to use a service worker to verify the client. Here is a paper which talks about this technique:

https://arxiv.org/pdf/2105.05551.pdf

If we were to implement this, we would have to be very careful to do it correctly. Service workers aren't really intended for this use case and the server can normally override the service worker. But the authors of the paper above claim that this would be detectable. We'd have to make sure that that holds true based on our own analysis.

### Option 3: Bookmarklet

It is possible to craft a `data:` URL which contains a small piece of JavaScript which can load a more complex script via hash. This can then in turn load/verify a more complex application as needed. Unfortunately, `data:` origins can't store data and a lot of web features are disabled, so it wouldn't be possible to load the entire application in such a context. But it could potentially be combined with the service worker approach to verify that the correct service worker code has been loaded.

## Content Security Policy

The Dassie frontend could benefit from a Content Security Policy to prevent some types of attacks like cross-site scripting (XSS).
