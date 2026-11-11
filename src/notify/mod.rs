//! Native notification transports. Transport acceptance is never an inbox read.
mod transport;
pub(crate) use transport::{Delivery, Target, deliver};
mod state;
mod worker;
pub(crate) use state::{State, now};
pub(crate) use worker::{Report, bound_member_ids, inbox, once};
