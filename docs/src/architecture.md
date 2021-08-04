# Architectural Overview

## Composition and Substitution

Trillium is published as a set of components that can be easily composed
to create web servers. One of the goals of this design is that to the
extent possible, all components be replaceable by alternatives.

## Why is substitution so important?

Async rust web frameworks still have a lot of exciting exploration
left in the near future. Instead of offering one solution as the best,
trillium offers a playground in which you can experiment with
alternatives. I want it to be painless to plug in an alternative
router, or a different http logger, or anything else you can imagine.

There are a lot of different purposes a web framework might be used
for, and the core library should not have to adapt in order for
someone to add support for each of those features.

Although I imagine that for each of the core components there will
only be one or two options, I think it is an essential aspect of good
software design that frameworks be modular and composable, as there
will always be tradeoffs for any given design.

## Only compile what you need

Instead of your application depending on a library with a large list
of reexported dependencies and conditionally including/excluding them
based on cargo features, trillium tries to apply rust's "only pay for
what you need" approach both at runtime and compile time.  In
particular, trillium avoids pulling in runtimes like tokio or
async-std except in the crates where you explicitly need those,
preferring instead to depend on small crates like `futures_lite`
wherever possible. Additionally, and in specific contrast to tide,
there is minimal default behavior. If you don't need a router, you
don't need to compile or run a router.

Everything is opt-in, instead of opt-out. Trillium uses small crates,
each of which declares its own dependencies.

### Relation to tide, http-types, and async-h1

As of trillium-v0.2.0, trillium no longer depends on http-types.

Trillium shares the same session store backends as tide.


### Relation to Elixir Plug and Phoenix

The general architecture is directly inspired by Plug, and is intended
to be a hybrid of the best of plug and the best of tide. Eventually, I
intend to build an opinionated framework like Phoenix on top of the
components that are Trillium, but I don't expect that to happen for a
bit. I hope to keep the core feature set of trillium quite small and
focus on getting the design right and improving performance as much as
possible. 
