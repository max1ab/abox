//! Native notification transports. Transport acceptance is never an inbox read.
mod transport;
pub(crate) use transport::{Delivery, Target, deliver};
mod state;
mod worker;
pub(crate) use state::State;
pub(crate) use worker::{inbox, once};
