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

pub struct DummySink<O: Unpin> {
    value: Arc<Mutex<Option<O>>>,
}
impl<O: Unpin> DummySink<O> {
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
            Some(_) => std::task::Poll::Pending,
            None => {
                // XXX Waker
                std::task::Poll::Ready(Ok(()))
            }
        };
        dbg!(poll_ready)
    }

    fn start_send(self: std::pin::Pin<&mut Self>, item: O) -> Result<(), Self::Error> {
        let start_send = self.value.lock().replace(item).map_or(Ok(()), |_| Err(()));
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

// impl<O: Unpin> Stream for SinkStream<O> {
//     type Item = O;

//     fn poll_next(
//         mut self: std::pin::Pin<&mut Self>,
//         cx: &mut std::task::Context<'_>,
//     ) -> std::task::Poll<Option<Self::Item>> {
//         match self.value.lock().take() {
//             Some(v) => Poll::Ready(Some(v)),
//             None => {
//                 // XXX waker
//                 Poll::Pending
//             }
//         }
//     }
// }

pub struct SinkStepStream<F: FusedFuture + Unpin, O: Unpin> {
    step: F,
    ch: DummySink<O>,
}

impl<F, O, A> Stream for SinkStepStream<F, O>
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
        if !self.step.is_terminated() {
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

impl<F, O> SinkStepStream<F, O>
where
    F: FusedFuture + Unpin,
    O: Unpin,
{
    pub fn new<F2>(f2: F2) -> Self
    where
        F2: FnOnce(DummySink<O>) -> F,
    {
        let (tx, ch) = DummySink::new();
        Self { step: f2(tx), ch }
    }
}

impl<F, O> SinkStepStream<Pin<Box<Fuse<F>>>, O>
where
    F: Future,
    O: Unpin,
{
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

    type D = Vec<String>;
    async fn test_f<F, Fut>(f: F, mut data: D)
    where
        F: Fn(DummySink<String>, D) -> Fut,
        Fut: Future<Output = ()>,
    {
        let stream = SinkStepStream::boxed(|sink| f(sink, data.clone()));

        tokio::time::timeout(
            Duration::from_millis(100),
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
