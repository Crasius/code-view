//! What the app does with raw pointer input before and after the pages see it, the same on every
//! page: wheel notches glide ([`smooth_wheel`]), a middle click scrolls ([`autoscroll`]) and
//! everything clickable shows the pointing hand (`cv_base::platform::pointer`).

pub mod autoscroll;
pub mod smooth_wheel;
