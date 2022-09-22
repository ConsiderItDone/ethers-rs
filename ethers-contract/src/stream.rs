use crate::LogMeta;
use ethers_core::types::{Log, U256};
use futures_util::{
    future::Either,
    stream::{Stream, StreamExt},
};
use pin_project::pin_project;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

type MapEvent<'a, R, E> = Box<dyn Fn(Log) -> Result<R, E> + 'a + Send + Sync>;

#[pin_project]
/// Generic wrapper around Log streams, mapping their content to a specific
/// deserialized log struct.
///
/// We use this wrapper type instead of `StreamExt::map` in order to preserve
/// information about the filter/subscription's id.
pub struct EventStream<'a, T, R, E> {
    pub id: U256,
    #[pin]
    stream: T,
    parse: MapEvent<'a, R, E>,
}

impl<'a, T, R, E> EventStream<'a, T, R, E> {
    /// Turns this stream of events into a stream that also yields the event's metadata
    pub fn with_meta(self) -> EventStreamMeta<'a, T, R, E> {
        EventStreamMeta(self)
    }
}

impl<'a, T, R, E> EventStream<'a, T, R, E> {
    pub fn new(id: U256, stream: T, parse: MapEvent<'a, R, E>) -> Self {
        Self { id, stream, parse }
    }
}

impl<'a, T, R, E> Stream for EventStream<'a, T, R, E>
where
    T: Stream<Item = Log> + Unpin,
{
    type Item = Result<R, E>;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        match futures_util::ready!(this.stream.poll_next_unpin(ctx)) {
            Some(item) => Poll::Ready(Some((this.parse)(item))),
            None => Poll::Pending,
        }
    }
}

impl<'a, T, R, E> EventStream<'a, T, R, E>
where
    T: Stream<Item = Log> + Unpin + 'a,
    R: 'a,
    E: 'a,
{
    pub fn select<St>(self, st: St) -> SelectEvent<SelectEither<'a, Result<R, E>, St::Item>>
    where
        St: Stream + Unpin + 'a,
    {
        SelectEvent(Box::pin(futures_util::stream::select(
            self.map(Either::Left),
            st.map(Either::Right),
        )))
    }
}

pub type SelectEither<'a, L, R> = Pin<Box<dyn Stream<Item = Either<L, R>> + 'a>>;

#[pin_project]
pub struct SelectEvent<T>(#[pin] T);

impl<'a, T, L, LE, R, RE> SelectEvent<T>
where
    T: Stream<Item = Either<Result<L, LE>, Result<R, RE>>> + 'a,
    L: 'a,
    LE: 'a,
    R: 'a,
    RE: 'a,
{
    /// Turns a stream of Results to a stream of `Result::ok` for both arms
    pub fn ok(self) -> Pin<Box<dyn Stream<Item = Either<L, R>> + 'a>> {
        Box::pin(self.filter_map(|e| async move {
            match e {
                Either::Left(res) => res.ok().map(Either::Left),
                Either::Right(res) => res.ok().map(Either::Right),
            }
        }))
    }
}

impl<T: Stream> Stream for SelectEvent<T> {
    type Item = T::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        this.0.poll_next(cx)
    }
}

/// Wrapper around a `EventStream`, that in addition to the deserialized Event type also yields the
/// `LogMeta`.
#[pin_project]
pub struct EventStreamMeta<'a, T, R, E>(pub EventStream<'a, T, R, E>);

impl<'a, T, R, E> EventStreamMeta<'a, T, R, E>
where
    T: Stream<Item = Log> + Unpin + 'a,
    R: 'a,
    E: 'a,
{
    /// See `EventStream::select`
    #[allow(clippy::type_complexity)]
    pub fn select<St>(
        self,
        st: St,
    ) -> SelectEvent<SelectEither<'a, Result<(R, LogMeta), E>, St::Item>>
    where
        St: Stream + Unpin + 'a,
    {
        SelectEvent(Box::pin(futures_util::stream::select(
            self.map(Either::Left),
            st.map(Either::Right),
        )))
    }
}

impl<'a, T, R, E> Stream for EventStreamMeta<'a, T, R, E>
where
    T: Stream<Item = Log> + Unpin,
{
    type Item = Result<(R, LogMeta), E>;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match futures_util::ready!(this.0.stream.poll_next_unpin(ctx)) {
            Some(item) => {
                let meta = LogMeta::from(&item);
                let res = (this.0.parse)(item);
                let res = res.map(|inner| (inner, meta));
                Poll::Ready(Some(res))
            }
            None => Poll::Ready(None),
        }
    }
}
