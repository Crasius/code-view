//! What the pull request overview keeps between runs: the repositories starred for its Favorites
//! lane ([`favorites`]), and how its cards group their rows by repository ([`sections`]).

pub(crate) mod favorites;
mod sections;

pub(crate) use favorites::FavoriteRepository;
pub use favorites::Favorites;
pub use sections::{RepositorySections, SectionCard, SectionKey};
