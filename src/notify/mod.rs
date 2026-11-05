//! Native notification transports. Transport acceptance is never an inbox read.
mod transport;
pub(crate) use transport::{Delivery, Target, deliver};
