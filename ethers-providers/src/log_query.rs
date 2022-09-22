use super::{JsonRpcClient, Middleware, PinBoxFut, Provider, ProviderError};
use ethers_core::types::{Filter, Log, U64};
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};
use thiserror::Error;

pub struct LogQuery<'a, P> {
    provider: &'a Provider<P>,
    filter: Filter,
    from_block: Option<U64>,
    page_size: u64,
    current_logs: VecDeque<Log>,
    last_block: Option<U64>,
    state: LogQueryState,
}

enum LogQueryState {
    Initial,
    LoadLastBlock(PinBoxFut<U64>),
    LoadLogs(PinBoxFut<Vec<Log>>),
    Consume,
}

impl<'a, P> LogQuery<'a, P>
where
    P: JsonRpcClient,
{
    pub fn new(provider: &'a Provider<P>, filter: &Filter) -> Self {
        Self {
            provider,
            filter: filter.clone(),
            from_block: filter.get_from_block(),
            page_size: 10000,
            current_logs: VecDeque::new(),
            last_block: None,
            state: LogQueryState::Initial,
        }
    }

    /// set page size for pagination
    pub fn with_page_size(mut self, page_size: u64) -> Self {
        self.page_size = page_size;
        self
    }
}

macro_rules! rewake_with_new_state {
    ($ctx:ident, $this:ident, $new_state:expr) => {
        $this.state = $new_state;
        $ctx.waker().wake_by_ref();
        return Poll::Pending
    };
}

#[derive(Error, Debug)]
pub enum LogQueryError<E> {
    #[error(transparent)]
    LoadLastBlockError(E),
    #[error(transparent)]
    LoadLogsError(E),
}
