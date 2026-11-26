//! Code View's runtime, below the media, Markdown, services and every page: the [`background`]
//! tasks and [`load`]s that keep the UI thread from waiting, the [`refresh`] schedule, the
//! [`cache`]s with their memory budgets, the [`toasts`] with their ways out, where the app is and
//! was ([`route`], [`history`], [`trail`]), what a link points at ([`links`]) and the mailboxes
//! one page leaves for another ([`intents`]).
#![forbid(unsafe_code)]

pub mod background;
pub mod cache;
pub mod history;
pub mod intents;
pub mod links;
pub mod load;
pub mod refresh;
pub mod route;
pub mod toasts;
pub mod trail;
