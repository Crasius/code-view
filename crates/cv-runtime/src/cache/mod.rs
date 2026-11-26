//! What the app keeps to show again without asking: the memory budgets and holders every cache
//! counts under ([`memory`], with the cache policy), the last answers of lists and pull request
//! parts ([`page_cache`]) and the disk tier behind them ([`cold`]).

pub mod cold;
pub mod memory;
pub mod page_cache;
