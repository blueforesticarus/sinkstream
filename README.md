OBVIOUS WARNING: this is broken as fuck, do not use.

This is an attempt at converting functions which take a `futures::Sink` into ones which produce a `futures::Stream`
ie. we have
```rust
async fn foo1<S: futures::Sink<X>>(sink: S)
```
but we want
```rust
fn foo2() -> impl futures::Steam<Item = X>
```

Going from the latter to the former is easy.
```rust
    async fn foo1<S: futures::Sink<X>(sink: S)
    {
        use futures::StreamExt;
        foo2.map(Ok).forward(sink).await;
    }
```

But I have yet to see anything for doing the opposite. 

Motivations include:
1. Cleanliness : I find that defining functions and structs genericly over Sink is a good way to keep interface seperate from logic. (impl Stream also facilitates this)
2. Symmetry : The 2 forms of foo above are conceptually similar, being able to convert between them makes that translate into practice
3. Flexibility : Its hard to know which form the caller will prefer. (For this reason I currently prefer the Stream style, as I can always forward to a Sink).
4. Clarity : The Sink style is normally more imperative, and in may cases clearer. Stream combinators can be tricky with all the borrows/moves.
