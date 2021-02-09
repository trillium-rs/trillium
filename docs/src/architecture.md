# Architectural Overview

## Composability

Myco is published as a set of components that can be easily composed
to create web servers. One of the goals of this design is that to the
extent possible, all components be replaceable by alternatives.

## Why is replacability so important?

It is the author's opinion that async rust web frameworks still have a
lot of exciting exploration left in the near future. Instead of
offering one solution as the best, myco offers a playground in which
you can experiment with alternatives. I want it to be painless to plug
in an alternative router, or a different http logger, or anything else
you can imagine.

There are a lot of different purposes a web framework might be used
for, and the core library should not have to adapt in order for
someone to add support for each of those features.

Although I imagine that for each of the core components there will
only be one or two options, I think it is an essential aspect of good
software design that frameworks be modular and composable, as there
will always be tradeoffs for any given design.

## Only compile what you need, without having to toggle cargo features

Instead of declaring a large list of top level dependencies and
conditionally including/excluding them based on cargo features, myco
has a "only compile what you need" approach to dependencies.  In
particular, we avoid pulling in runtimes like tokio or async-std
except in the crates where you explicitly need those, preferring
instead to depend on small crates like `futures_lite` wherever
possible.

Everything is opt-in, instead of opt-out. We use small crates, each of
which declares its own dependencies.

### Relation to tide, http-types, and async-h1

Currently, myco uses http-types for several core types, like headers,
status codes, response bodies, and the conn state type map. Myco
sessions also shares the same session store backends as
tide. Currently, myco reuses several types from async-h1, but does not
depend on the crate in order to avoid pulling in unnecessary
dependencies.


### Relation to Elixir Plug and Phoenix

The general architecture is directly inspired by Plug, and is intended
to be a hybrid of the best of plug and the best of tide. Eventually, I
hope to build an opinionated framework like Phoenix on top of the
components that are myco, but I don't expect that to happen for a
while. I hope to keep the core feature set of myco quite small and
focus on getting the design right and improving performance as much as
possible. 
