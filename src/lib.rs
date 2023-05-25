//! This is an attempt at converting functions which take a `futures::Sink` into ones which produce a `futures::Stream`
//! ie. we have
//! ```text
//! async fn foo<S: futures::Sink<()>>(sink: S);
//! ```
//! but we want
//! ```text
//! async fn foo<S: futures::Sink<()>>(sink: S);
//! ```

use std::{
    fmt::Debug,
    pin::Pin,
    sync::Arc,
    task::Poll,
};

use futures::{
    future::{
        Fuse,
        FusedFuture,
    },
    Future,
    FutureExt,
    Sink,
    Stream,
};
use parking_lot::Mutex;

/// The Sink which gets passed to our function, holds a single value.
pub struct DummySink<O: Unpin> {
    value: Arc<Mutex<Option<O>>>,
}
impl<O: Unpin> DummySink<O> {
    /// Both the SyncStream it's driving future need a copy of this, so return 2.
    /// TODO: There is probably something better/more clever than an Arc<Mutex>
    fn new() -> (Self, Self) {
        let value: Arc<Mutex<Option<O>>> = Default::default();
        (
            Self {
                value: value.clone(),
            },
            Self { value },
        )
    }
}

impl<O: Unpin> Sink<O> for DummySink<O> {
    type Error = ();

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let poll_ready = match self.value.lock().as_ref() {
            Some(_) => {
                // *I think* waker is not needed. This only occurs when:
                // 1. SinkStream::poll_next drives forward the Future (DummySink guarenteed to be empty at this point)
                // 2. The future puts something in the DummySink (will succeed)
                // 3. The future wants to put another item in the DummySink (will fail, poll_ready will return Pending here)
                // 4. The future will return Pending since it is now waiting for DummySink.
                // 5. control flow returns to SinkStream::poll_next, which checks the DummySink and returns Poll::Ready(Some(value))
                // 6. *unsure of this*: Since SinkStream returned a value, whatever is consuming it will call SinkSteam::poll_next again.
                //XXX: What I am unsure of is whether wakers have any effect when Ready is returned. Ie. could some waker thing prevent SinkSteam::poll_next from being called again after it returned Poll:Ready?
                //XXX: My understanding is that wakers only schedule when to repoll a *Pending* stream, and that when Ready is returned, it will keep getting consumed. (My understanding of how futures and streams interface is a bit shacky.)
                std::task::Poll::Pending
            }
            None => std::task::Poll::Ready(Ok(())),
        };
        dbg!(poll_ready)
    }

    fn start_send(self: std::pin::Pin<&mut Self>, item: O) -> Result<(), Self::Error> {
        let start_send = self
            .value
            .lock()
            .replace(item) // Store the value in the Option
            // *I think* it is impossible for there to already be something in there, since poll_ready would have returned Pending
            .map_or(Ok(()), |_| {
                unreachable!("start_send to non-empty DummySink")
            });
        dbg!(start_send)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let poll_flush = self.poll_ready(cx);
        dbg!(poll_flush)
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let poll_ready = self.poll_ready(cx);
        dbg!(poll_ready)
    }
}

/// The wrapper type implementing Stream.
pub struct SinkStream<F: FusedFuture + Unpin, O: Unpin> {
    /// The future which is sending values to the DummySink
    step: F,

    /// The DummySink. We give this to the future in New.
    ch: DummySink<O>,
}

impl<F, O, A> Stream for SinkStream<F, O>
where
    F: FusedFuture<Output = A> + Unpin,
    O: Unpin + Debug,
    A: Debug,
{
    type Item = O;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        //values can only be added by the future, and are removed every time after it is polled.
        //XXX: Possible issues if the future is able to "give" the Sink to another thread/task
        assert!(self.ch.value.lock().is_none());

        if !self.step.is_terminated() {
            // This will drive forward the future. There are 4 things that can happen.
            // 1. The future blocks on something
            // 2. The future puts something in the DummySink then blocks on something else
            // 3. The future puts something in the DummySink then blocks putting another value in
            // 4. The future puts something in the DummySink then exits/resolves
            // 5. The future exits/resolves.

            // In cases 1 and 2, whatever the future blocks on will register their own waker with the context.
            // In case 1, this function returns Pending so no issue.
            // In case 2, this function returns Ready, will that clobber the waker stuff?

            // In all other cases there is no waker, and this function returns Ready
            // In case 3, this function returns Ready(Some(item)), on next invocation the future will poll_ready and insert the next item.
            // In case 4, this function returns Ready(Some(item)), on next invocation the future will be terminated and we return Ready(None)
            // In case 5, this function returns Ready(None)

            let step = self.as_mut().step.poll_unpin(cx);
            dbg!(&step);
        }

        let poll_next = match self.ch.value.lock().take() {
            Some(v) => Poll::Ready(Some(v)),
            None => match self.step.is_terminated() {
                false => Poll::Pending,
                true => Poll::Ready(None),
            },
        };

        dbg!(poll_next)
    }
}

impl<F, O> SinkStream<F, O>
where
    F: FusedFuture + Unpin,
    O: Unpin,
{
    /// Create a SinkStream by passing a new DummySink to an async fn
    pub fn new<F2>(f2: F2) -> Self
    where
        F2: FnOnce(DummySink<O>) -> F,
    {
        let (tx, ch) = DummySink::new();
        Self { step: f2(tx), ch }
    }
}

impl<F, O> SinkStream<Pin<Box<Fuse<F>>>, O>
where
    F: Future,
    O: Unpin,
{
    /// Helper function to fuse, box, and pin the future
    pub fn boxed<F2>(f2: F2) -> Self
    where
        F2: FnOnce(DummySink<O>) -> F,
    {
        Self::new(|sink| Box::pin(f2(sink).fuse()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::{
        SinkExt,
        StreamExt,
    };
    use itertools::Itertools;

    use super::*;

    #[tokio::test]
    /// The send all items without doing anything else
    pub async fn test_no_sleep() {
        pub async fn foo<S>(sink: S, items: Vec<String>)
        where
            S: futures::Sink<String>,
        {
            let mut sink = Box::pin(sink);
            for item in items {
                dbg!(&item);
                sink.feed(item).await.map_err(|_| "err").unwrap();
            }
        }

        let data: Vec<String> = "a b c d"
            .split_ascii_whitespace()
            .map(ToString::to_string)
            .collect();
        test_f(foo, data).await;
    }

    #[tokio::test]
    /// The most basic test: sleep, send, repeat
    pub async fn test_one_by_one() {
        pub async fn foo<S>(sink: S, items: Vec<String>)
        where
            S: futures::Sink<String>,
        {
            let mut sink = Box::pin(sink);
            for item in items {
                dbg!("sleep");
                tokio::time::sleep(Duration::from_millis(1)).await;
                dbg!(&item);
                sink.feed(item).await.map_err(|_| "err").unwrap();
            }
        }

        let data: Vec<String> = "hello world"
            .split_ascii_whitespace()
            .map(ToString::to_string)
            .collect();
        test_f(foo, data).await;
    }

    #[tokio::test]
    /// Test sending multible items between sleep
    pub async fn test_multi() {
        pub async fn foo<S>(sink: S, items: Vec<String>)
        where
            S: futures::Sink<String>,
        {
            let mut sink = Box::pin(sink);
            for items in &items.into_iter().chunks(3) {
                dbg!("sleep");
                tokio::time::sleep(Duration::from_millis(1)).await;
                let items = dbg!(items.collect_vec());
                for item in items {
                    dbg!(&item);
                    sink.feed(item).await.map_err(|_| "err").unwrap();
                }
            }
        }

        let data: Vec<String> = "a b c d e f g"
            .split_ascii_whitespace()
            .map(ToString::to_string)
            .collect();
        test_f(foo, data).await;
    }

    #[tokio::test]
    /// Test if waiting on multible futures between sends.
    pub async fn test_multi_sleep() {
        pub async fn foo<S>(sink: S, items: Vec<String>)
        where
            S: futures::Sink<String>,
        {
            let mut sink = Box::pin(sink);
            for item in items {
                dbg!("sleep");
                tokio::time::sleep(Duration::from_millis(1)).await;
                tokio::time::sleep(Duration::from_millis(1)).await;
                dbg!(&item);
                sink.feed(item).await.map_err(|_| "err").unwrap();
            }
        }

        let data: Vec<String> = "hello world"
            .split_ascii_whitespace()
            .map(ToString::to_string)
            .collect();
        test_f(foo, data).await;
    }

    #[tokio::test]
    /// Test if waiting on multible futures between sends.
    pub async fn test_no_op() {
        pub async fn foo<S>(_: S, _: Vec<String>)
        where
            S: futures::Sink<String>,
        {
        }

        let data: Vec<String> = vec![];
        test_f(foo, data).await;
    }

    #[tokio::test]
    /// Test if waiting on multible futures between sends.
    pub async fn test_just_sleep() {
        pub async fn foo<S>(_: S, _: Vec<String>)
        where
            S: futures::Sink<String>,
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let data: Vec<String> = vec![];
        test_f(foo, data).await;
    }

    type D = Vec<String>;
    async fn test_f<F, Fut>(f: F, mut data: D)
    where
        F: Fn(DummySink<String>, D) -> Fut,
        Fut: Future<Output = ()>,
    {
        let stream = SinkStream::boxed(|sink| f(sink, data.clone()));

        tokio::time::timeout(
            Duration::from_millis(200),
            stream
                .map(|got| {
                    assert_eq!(got, data.remove(0));
                    dbg!(got)
                })
                .collect::<Vec<String>>(),
        )
        .await
        .expect("timeout");

        assert_eq!(data, Vec::<String>::new(), "unprocessed items")
    }
}
